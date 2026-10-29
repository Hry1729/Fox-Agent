import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { SessionManager, fauxAssistantMessage, registerFauxProvider, createFoxModelRuntime, createFoxAgentSession,
  SettingsManager, DefaultResourceLoader } from '../src/pi-adapter.mjs'
import { markNativeSessionRun, nativeSessionToolCalls, reconcileNativeSession, TOOL_CALL_FACTS } from '../src/pi-native-recovery.mjs'
import { nativeToolInputDigest, nativeHostToolResult, NATIVE_RESPONSE_DIGEST } from '../src/pi-native-tool-bridge.mjs'

const generation = 3
let providerSequence = 0
const calls = [
  { type: 'toolCall', id: 'read-1', name: 'read', arguments: { path: 'one' } },
  { type: 'toolCall', id: 'work-2', name: 'work_snapshot_get', arguments: {} },
]
const textResult = (call, text) => ({ role: 'toolResult', toolCallId: call.id, toolName: call.name,
  isError: false, content: [{ type: 'text', text }], timestamp: Date.now() })
const completed = (callId, text) => ({ callId, state: 'committed', response: { result: { content: [{ type: 'text', text }] } } })
const notExecuted = callId => ({ callId, state: 'not_executed', response: {
  isError: true, error: 'This registered call did not execute.', errorDetails: { code: 'runtime.tool_not_started', retryable: false },
} })

async function fixture(t, { results = [], aborted = false, legacy = false, priorResult = false } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-recovery-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const manager = SessionManager.create(directory, join(directory, 'sessions'))
  if (priorResult) {
    const prior = { type: 'toolCall', id: 'previous-call', name: 'skill_load', arguments: {} }
    markNativeSessionRun(manager, { runId: 'previous-run', contextGeneration: generation, executorRoute: 'pi_native' })
    manager.appendMessage({ role: 'user', content: [{ type: 'text', text: 'Prior completed turn.' }], timestamp: Date.now() })
    manager.appendMessage(fauxAssistantMessage([prior], { stopReason: 'toolUse' }))
    manager.appendMessage(textResult(prior, 'Prior completed result.'))
  }
  if (!legacy) markNativeSessionRun(manager, { runId: 'original-run', contextGeneration: generation, executorRoute: 'pi_native' })
  manager.appendMessage({ role: 'user', content: [{ type: 'text', text: 'Read the source and work snapshot.' }], timestamp: Date.now() })
  manager.appendMessage(fauxAssistantMessage(calls, { stopReason: 'toolUse' }))
  for (const index of results) manager.appendMessage(textResult(calls[index], 'original result'))
  if (aborted) manager.appendMessage(fauxAssistantMessage('interrupted', { stopReason: 'aborted' }))
  return { directory, manager, path: manager.getSessionFile() }
}

function reconcile(manager, facts, extra = {}) {
  return reconcileNativeSession({
    manager, runId: 'recovery-run', contextGeneration: generation,
    requestHost: async (type, payload) => {
      assert.equal(type, TOOL_CALL_FACTS)
      assert.equal(payload.runId, 'recovery-run')
      assert.equal(payload.contextGeneration, generation)
      assert.deepEqual(payload.calls.map(({ callId, inputDigest }) => ({ callId, inputDigest })),
        calls.map(call => ({ callId: call.id, inputDigest: nativeToolInputDigest(call.arguments) })))
      return { payload: { contextGeneration: generation, blocked: false, facts } }
    },
    ...extra,
  })
}

async function providerContextAfterRecovery(t, manager) {
  const name = `faux-recovered-context-${++providerSequence}`
  const provider = registerFauxProvider({ api: name, provider: name, models: [{ id: name }], tokensPerSecond: 10000 })
  provider.setResponses([fauxAssistantMessage('The recovered receipt is the evidence.')])
  const model = provider.getModel()
  const modelRuntime = await createFoxModelRuntime({ model, fauxRegistration: provider })
  const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false, maxRetries: 0, provider: { maxRetries: 0 } } })
  const cwd = manager.getCwd()
  const resourceLoader = new DefaultResourceLoader({ cwd, agentDir: cwd, settingsManager, systemPrompt: 'Use the recovered Host evidence.',
    noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true })
  await resourceLoader.reload()
  const { session } = await createFoxAgentSession({ cwd, agentDir: cwd, model, modelRuntime, settingsManager, resourceLoader,
    sessionManager: manager, thinkingLevel: 'off', tools: [] })
  t.after(async () => { await session.abort(); session.dispose(); provider.unregister() })
  const stream = session.agent.streamFunction.bind(session.agent)
  let observed
  session.agent.streamFunction = (model, context, options) => { observed = context; return stream(model, context, options) }
  await session.prompt('Which effect actually happened?', { expandPromptTemplates: false })
  return observed
}

test('recovery appends committed and durably not-executed results once and survives reopen', async t => {
  const { manager, path } = await fixture(t)
  const facts = [completed('read-1', 'Host retained source'), notExecuted('work-2')]
  assert.deepEqual(await reconcile(manager, facts), { checked: 2, appended: 2 })
  const reopened = SessionManager.open(path)
  assert.deepEqual(await reconcile(reopened, facts), { checked: 2, appended: 0 })
  const results = reopened.getBranch().filter(entry => entry.message?.role === 'toolResult').map(entry => entry.message)
  assert.equal(results.length, 2)
  assert.equal(results[0].content[0].text, 'Host retained source')
  assert.equal(results[1].isError, true)
  assert.match(results[1].content[0].text, /did not execute/)
  assert.match(results[1].content[0].text, /runtime\.tool_not_started/)
})

test('real SDK manual compaction retains native evidence and required context after closing and reopening', async t => {
  const { directory, manager, path } = await fixture(t)
  const facts = [completed('read-1', 'U08_HOST_SOURCE: verified source'), completed('work-2', 'U08_HOST_WORK: retained work state')]
  await reconcile(manager, facts)
  manager.appendMessage(fauxAssistantMessage('The source and work results are complete.'))
  for (let index = 0; index < 6; index += 1) {
    manager.appendMessage({ role: 'user', content: [{ type: 'text', text: `U08_OLD_FILLER_${index}: ${'source evidence '.repeat(350)}` }], timestamp: Date.now() })
    manager.appendMessage(fauxAssistantMessage(`Checked ${index}: ${'verified evidence '.repeat(200)}`))
  }
  manager.appendMessage({ role: 'user', content: [{ type: 'text', text: 'U08_RECENT_CONSTRAINT: keep the original project scope.' }], timestamp: Date.now() })
  manager.appendMessage(fauxAssistantMessage('The recent project constraint remains active.'))
  const name = `faux-native-compaction-reopen-${++providerSequence}`
  const provider = registerFauxProvider({ api: name, provider: name,
    models: [{ id: name, contextWindow: 16384, maxTokens: 512 }], tokensPerSecond: 10000 })
  provider.setResponses(Array.from({ length: 2 }, () => fauxAssistantMessage(
    'U08_COMPACT_SUMMARY: U08_HOST_SOURCE verified source and U08_HOST_WORK retained work state.')))
  t.after(() => provider.unregister())
  const model = provider.getModel()
  const modelRuntime = await createFoxModelRuntime({ model, fauxRegistration: provider })
  const settingsManager = SettingsManager.inMemory({
    compaction: { enabled: true, reserveTokens: 512, keepRecentTokens: 128 },
    retry: { enabled: false, maxRetries: 0, provider: { maxRetries: 0 } },
  })
  const resourceLoader = new DefaultResourceLoader({ cwd: directory, agentDir: directory, settingsManager,
    systemPrompt: 'Summarize the actual source and work evidence.', noExtensions: true, noSkills: true,
    noPromptTemplates: true, noThemes: true, noContextFiles: true })
  await resourceLoader.reload()
  const { session } = await createFoxAgentSession({ cwd: directory, agentDir: directory, model, modelRuntime,
    settingsManager, resourceLoader, sessionManager: manager, thinkingLevel: 'off', tools: [] })
  const compactionContexts = []
  const stream = session.agent.streamFunction.bind(session.agent)
  session.agent.streamFunction = (model, context, options) => {
    compactionContexts.push(JSON.stringify(context))
    return stream(model, context, options)
  }
  let savedCompaction
  try {
    const compacted = await session.compact()
    savedCompaction = manager.getBranch().findLast(entry => entry.type === 'compaction')
    assert.ok(savedCompaction, 'the SDK itself persisted the compaction entry')
    assert.equal(savedCompaction.summary, compacted.summary)
    assert.equal(savedCompaction.firstKeptEntryId, compacted.firstKeptEntryId)
    assert.ok(compacted.tokensBefore > 0)
    assert.match(compacted.summary, /U08_COMPACT_SUMMARY/)
    assert.ok(compactionContexts.some(context => context.includes('U08_HOST_SOURCE')), 'actual summary requests contain the Host evidence')
  } finally {
    await session.abort()
    session.dispose()
  }
  const persisted = await readFile(path, 'utf8')
  const reopened = SessionManager.open(path)
  assert.equal(reopened.getSessionId(), manager.getSessionId())
  assert.deepEqual(reopened.getBranch().findLast(entry => entry.type === 'compaction'), savedCompaction)
  assert.deepEqual(nativeSessionToolCalls(reopened, generation).calls.map(call => ({ callId: call.callId, hasResult: call.hasResult })),
    calls.map(call => ({ callId: call.id, hasResult: true })))
  assert.deepEqual(await reconcile(reopened, facts), { checked: 2, appended: 0 })
  assert.equal(await readFile(path, 'utf8'), persisted, 'opening and fact reconciliation leave the compacted file intact')
  const nextContext = JSON.stringify(await providerContextAfterRecovery(t, reopened))
  assert.ok(nextContext.includes('U08_COMPACT_SUMMARY'), 'the reopened SDK sends the retained summary to the next model request')
  assert.ok(nextContext.includes('U08_HOST_SOURCE'))
  assert.ok(nextContext.includes('U08_HOST_WORK'))
  assert.ok(nextContext.includes('U08_RECENT_CONSTRAINT'), 'the kept recent requirement survives closing and reopening')
  assert.ok(!nextContext.includes('U08_OLD_FILLER_0'), 'summarized history is not replayed as the full old context')
})

test('recovery rebuilds the final partial batch and retains the aborted branch as history', async t => {
  const { manager, path } = await fixture(t, { results: [0], aborted: true })
  const abandoned = manager.getLeafId()
  await reconcile(manager, [completed('read-1', 'source'), notExecuted('work-2')])
  const reopened = SessionManager.open(path)
  assert.ok(reopened.getEntries().some(entry => entry.id === abandoned), 'the aborted entry remains recoverable history')
  const active = reopened.getBranch()
  assert.ok(!active.some(entry => entry.id === abandoned), 'the active branch goes through the actual partial result')
  assert.deepEqual(active.filter(entry => entry.message?.role === 'toolResult').map(entry => entry.message.toolCallId), ['read-1', 'work-2'])
})

test('an aborted batch with no partial result keeps its own tool-call assistant when branching', async t => {
  const { manager, path } = await fixture(t, { priorResult: true, aborted: true })
  await reconcileNativeSession({ manager, runId: 'recovery-run', contextGeneration: generation,
    requestHost: async () => ({ payload: { contextGeneration: generation, blocked: false, facts: [
      completed('previous-call', 'Prior completed result.'), completed('read-1', 'source'), notExecuted('work-2'),
    ] } }),
  })
  const reopened = SessionManager.open(path)
  const recovered = nativeSessionToolCalls(reopened, generation).calls
  assert.deepEqual(recovered.map(call => call.callId), ['previous-call', 'read-1', 'work-2'])
  assert.ok(recovered.every(call => call.hasResult))
})

test('a local cancellation result is replaced with the entire final Host batch and its late effect receipt', async t => {
  const { manager, path } = await fixture(t)
  const facts = [completed('read-1', 'Original Host source'), {
    callId: 'work-2', state: 'committed', response: { isError: true,
      error: 'Run cancelled; retained output is evidence.', result: { content: [{ type: 'text', text: 'HOST_EFFECT_RECEIPT: completed before cancellation.' }] } },
  }]
  const originalGood = manager.appendMessage({ ...nativeHostToolResult('read', facts[0].response, { runId: 'original-run', toolCallId: 'read-1' }),
    role: 'toolResult', toolCallId: 'read-1', toolName: 'read', timestamp: Date.now() })
  const cancelled = manager.appendMessage({ ...textResult(calls[1], 'Host request was cancelled.'), isError: true })
  manager.appendMessage(fauxAssistantMessage('', { stopReason: 'aborted' }))
  assert.deepEqual(await reconcile(manager, facts), { checked: 2, appended: 2 })
  const reopened = SessionManager.open(path)
  const active = reopened.getBranch()
  assert.ok(!active.some(entry => entry.id === originalGood || entry.id === cancelled), 'the whole batch is rebuilt, not only the cancellation result')
  assert.ok(reopened.getEntries().some(entry => entry.id === cancelled), 'the local cancellation remains in abandoned history')
  const results = active.filter(entry => entry.message?.role === 'toolResult').map(entry => entry.message)
  assert.deepEqual(results.map(result => result.toolCallId), ['read-1', 'work-2'])
  assert.equal(results[0].content[0].text, 'Original Host source')
  assert.match(results[1].content[0].text, /HOST_EFFECT_RECEIPT/)
  for (let index = 0; index < results.length; index += 1) {
    assert.equal(results[index].details[NATIVE_RESPONSE_DIGEST], nativeToolInputDigest(facts[index].response))
  }
  assert.deepEqual(await reconcile(reopened, facts), { checked: 2, appended: 0 })
  const context = JSON.stringify(await providerContextAfterRecovery(t, reopened))
  assert.ok(context.includes('HOST_EFFECT_RECEIPT'), 'the real SDK provider request includes the recovered effect receipt')
  assert.ok(!context.includes('Host request was cancelled.'), 'abandoned local cancellation is absent from the active provider context')
})

test('a mismatching result followed by a later user or assistant blocks recovery without changing history', async t => {
  for (const role of ['user', 'assistant']) {
    const { manager, path } = await fixture(t)
    manager.appendMessage({ ...textResult(calls[0], 'Host request was cancelled.'), isError: true })
    manager.appendMessage(textResult(calls[1], 'original result'))
    manager.appendMessage(role === 'user'
      ? { role, content: [{ type: 'text', text: 'A later question.' }], timestamp: Date.now() }
      : fauxAssistantMessage('A later answer.'))
    const before = await readFile(path, 'utf8')
    await assert.rejects(reconcile(manager, [completed('read-1', 'Host retained evidence'), completed('work-2', 'original result')]),
      error => error.code === 'session.native_recovery_inconsistent')
    assert.equal(await readFile(path, 'utf8'), before)
  }
})

test('an existing Host result needs matching visible content and the saved response digest', async t => {
  const { manager, path } = await fixture(t)
  const facts = [completed('read-1', 'Original Host source'), completed('work-2', 'Original work state')]
  for (let index = 0; index < calls.length; index += 1) {
    const result = nativeHostToolResult(calls[index].name, facts[index].response, { runId: 'original-run', toolCallId: calls[index].id })
    if (index === 0) result.details[NATIVE_RESPONSE_DIGEST] = 'sha256:wrong'
    manager.appendMessage({ ...result, role: 'toolResult', toolCallId: calls[index].id, toolName: calls[index].name, timestamp: Date.now() })
  }
  manager.appendMessage(fauxAssistantMessage('The completed turn is already followed by an answer.'))
  const before = await readFile(path, 'utf8')
  await assert.rejects(reconcile(manager, facts), error => error.code === 'session.native_recovery_inconsistent')
  assert.equal(await readFile(path, 'utf8'), before)
})

test('partial tool calls in an aborted assistant remain blocked instead of appending orphan results', async t => {
  const { manager, path } = await fixture(t)
  const user = manager.getBranch().findLast(entry => entry.message?.role === 'user')
  manager.branch(user.id)
  manager.appendMessage(fauxAssistantMessage(calls, { stopReason: 'aborted' }))
  const before = await readFile(path, 'utf8')
  await assert.rejects(reconcile(manager, [notExecuted('read-1'), notExecuted('work-2')]),
    error => error.code === 'session.native_recovery_inconsistent' && /assistant itself is incomplete/.test(error.message))
  assert.equal(await readFile(path, 'utf8'), before)
})

for (const [state, code] of [['outcome_unknown', 'session.native_recovery_pending'], ['waiting_approval', 'session.native_waiting_approval']]) {
  test(`${state} blocks recovery before any append or branch`, async t => {
    const { manager, path } = await fixture(t, { aborted: true })
    const before = await readFile(path, 'utf8')
    const leaf = manager.getLeafId()
    await assert.rejects(reconcile(manager, [completed('read-1', 'source'), { callId: 'work-2', state }], {
      requestHost: async () => ({ payload: { contextGeneration: generation, blocked: true,
        facts: [completed('read-1', 'source'), { callId: 'work-2', state }] } }),
    }), error => error.code === code)
    assert.equal(manager.getLeafId(), leaf)
    assert.equal(await readFile(path, 'utf8'), before)
  })
}

test('a Host consistency error keeps Pi-only results untouched', async t => {
  const { manager, path } = await fixture(t, { results: [0, 1] })
  const before = await readFile(path, 'utf8')
  await assert.rejects(reconcile(manager, [], {
    requestHost: async (_type, payload) => {
      assert.ok(payload.calls.every(call => call.hasResult))
      return { payload: { isError: true, error: 'Pi result has no Host commit.',
        errorDetails: { code: 'runtime.native_recovery_inconsistent', retryable: false } } }
    },
  }), error => error.code === 'session.native_recovery_inconsistent')
  assert.equal(await readFile(path, 'utf8'), before)
})

test('legacy tool history is outside the managed Run boundary', async t => {
  const { manager } = await fixture(t, { legacy: true, results: [0, 1] })
  let queries = 0
  assert.deepEqual(await reconcileNativeSession({ manager, contextGeneration: generation,
    requestHost: () => { queries += 1 } }), { checked: 0, appended: 0 })
  assert.equal(queries, 0)
  markNativeSessionRun(manager, { runId: 'legacy-again', contextGeneration: generation, executorRoute: null })
  manager.appendMessage(fauxAssistantMessage([{ type: 'toolCall', id: 'legacy-extra', name: 'skill_load', arguments: {} }], { stopReason: 'toolUse' }))
  assert.deepEqual(nativeSessionToolCalls(manager, generation).calls, [])
})

test('a generation mismatch or missing Host identity cannot start recovery', async t => {
  const { manager } = await fixture(t)
  await assert.rejects(reconcileNativeSession({ manager, contextGeneration: generation }), error => error.code === 'session.native_recovery_identity_required')
  await assert.rejects(reconcileNativeSession({ manager, runId: 'recovery-run', contextGeneration: generation + 1 }), error => error.code === 'session.native_generation_mismatch')
})

for (const defect of ['missing', 'reordered', 'no-response']) {
  test(`a ${defect} Host fact set is rejected before any local mutation`, async t => {
    const { manager, path } = await fixture(t, { aborted: true })
    const before = await readFile(path, 'utf8')
    const facts = [completed('read-1', 'source'), notExecuted('work-2')]
    if (defect === 'missing') facts.pop()
    if (defect === 'reordered') facts.reverse()
    if (defect === 'no-response') delete facts[1].response
    await assert.rejects(reconcile(manager, facts), error => error.code === 'session.native_recovery_protocol')
    assert.equal(await readFile(path, 'utf8'), before)
  })
}

test('cancellation after Host facts arrives still leaves the Pi branch untouched', async t => {
  const { manager, path } = await fixture(t, { aborted: true })
  const before = await readFile(path, 'utf8')
  const controller = new AbortController()
  await assert.rejects(reconcile(manager, [], { signal: controller.signal, requestHost: async () => {
    controller.abort()
    return { payload: { contextGeneration: generation, blocked: false,
      facts: [completed('read-1', 'source'), notExecuted('work-2')] } }
  } }), error => error.name === 'AbortError')
  assert.equal(await readFile(path, 'utf8'), before)
})
