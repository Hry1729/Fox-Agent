import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { installKernelProposalTools, prepareKernelModelResponse, prepareKernelBatchResume, resumePiKernelBatch } from '../src/pi-kernel-batch-resume.mjs'
import { validatePromptControl } from '../src/control-binding.mjs'

const identity = { runId: 'kernel-run', conversationId: 'kernel-conversation', runtimeSessionId: 'kernel-session', executionProfileId: 'legacy' }

test('Host execution receipts survive provider projection without rewriting durable results', () => {
  const request = fixture()
  const receipt = 'FOX_EXECUTION_RECEIPT_V1\n' + JSON.stringify({source:'fox_kernel_host',approvalDecision:'allow_once',executionState:'completed',write:{exactSubmittedContent:'中文 😀\n',utf8Bytes:12}})
  request.payload.batchResume.tools[0].result.content.push({type:'text',text:receipt})
  const before = structuredClone(request)
  const prepared = prepareKernelBatchResume(request, identity)
  assert.equal(prepared.messages.at(-1).content.at(-1).text, receipt)
  assert.deepEqual(prepared.messages.at(-1).details, {})
  assert.deepEqual(request, before)
})

test('Kernel replay supplies local estimation metadata without changing durable facts or restoring provider diagnostics', () => {
  const request = fixture()
  request.payload.batchResume.assistantMessage.usage = { totalTokens: 999999 }
  request.payload.batchResume.assistantMessage.errorMessage = 'private provider diagnostic'
  const before = structuredClone(request)
  const prepared = prepareKernelBatchResume(request, identity)
  const assistant = prepared.messages.find(message => message.role === 'assistant')
  assert.deepEqual(assistant.usage, {
    input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  })
  assert.equal(assistant.errorMessage, undefined)
  assert.deepEqual(request, before)
})

test('model responses preserve required arguments while omitting absent optional Pi metadata', () => {
  const prepared = { runId: 'run-1', turnId: 'turn-1', batchId: 'batch-1', checkpointSeq: 8 }
  const answer = { role: 'assistant', stopReason: 'stop', content: [{ type: 'text', text: 'done' }], errorMessage: undefined }
  assert.equal(Object.hasOwn(prepareKernelModelResponse(answer, prepared).assistantMessage, 'errorMessage'), false)
  for (const message of [
    { ...answer, stopReason: 'length' }, { ...answer, stopReason: 'error' },
    { ...answer, content: [{ type: 'unknown', text: 'done' }] },
    { role: 'assistant', stopReason: 'toolUse', content: [] },
    { role: 'assistant', stopReason: 'toolUse', content: [{ type: 'toolCall', id: 'a', name: 'unknown', arguments: {} }] },
    { role: 'assistant', stopReason: 'toolUse', content: [{ type: 'toolCall', id: 'a', name: 'read', arguments: { path: undefined } }] },
  ]) assert.throws(() => prepareKernelModelResponse(message, prepared))
})

test('proposal schemas cannot retain or replace local resource executors', async () => {
  const session = controlledSession(async () => {})
  session.agent.subscribe = () => () => {}
  session.agent.abort = () => {}
  assert.throws(() => installKernelProposalTools(session, [{ name: 'unknown', description: 'bad', parameters: { type: 'object' } }]))
  installKernelProposalTools(session, [{ name: 'read', description: 'Host-only read', parameters: { type: 'object' },
    execute: () => { throw new Error('untrusted executor'); } }])
  await assert.rejects(session.agent.state.tools[0].execute(), /no resource executor/)
  session.agent.state.tools = [{ ...session.agent.state.tools[0], execute: async () => ({}) }]
  await assert.rejects(resumePiKernelBatch(session, fixture(), identity, new AbortController().signal), /executable local tools/)
})
function controlledSession(continueModel, abortModel = async () => {}) {
  const session = {
    isIdle: true, state: { isStreaming: false }, abort: abortModel,
    settingsManager: {
      getRetryEnabled: () => false, getRetrySettings: () => ({ maxRetries: 0 }),
      getProviderRetrySettings: () => ({ maxRetries: 0 }), getCompactionEnabled: () => false,
    },
    agent: { state: { messages: [], tools: [] }, continue: () => continueModel(session) },
  }
  return session
}
function fixture() {
  const permission = { mode: 'read_only', projectRoot: null, grants: [] }
  return { ...identity, payload: {
    controlBinding: {
      schemaVersion: 1, runId: identity.runId, conversationId: identity.conversationId,
      engineId: 'pi', executionProfileId: identity.executionProfileId, authority: 'authoritative', readOnlyExecutor: 'rust',
      permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`, permission,
      budgets: { modelRequestMs: 120_000, toolExecutionMs: 600_000, runExecutionMs: 1_800_000, approvalWaitMs: 300_000 },
    },
    batchResume: {
      schemaVersion: 1, turnId: 'turn-1', batchId: 'batch-1', idempotencyKey: 'tool-batch-delivery:batch-1', checkpointSeq: 8,
      history: [{ role: 'user', content: 'read both files', timestamp: 1 }],
      assistantMessage: { role: 'assistant', stopReason: 'toolUse', timestamp: 2, content: [
        { type: 'toolCall', id: 'read-a', name: 'read', arguments: { path: 'a.txt' } },
        { type: 'toolCall', id: 'read-b', name: 'read', arguments: { path: 'b.txt' } },
      ] },
      tools: ['b', 'a'].map((suffix, index) => ({
        toolCallId: `read-${suffix}`, tool: 'read', sourceOrder: 1 - index, canonicalInput: { path: `${suffix}.txt` }, state: 'completed',
        result: { content: [{ type: 'text', text: `durable result ${suffix}` }], details: { namespace: 'not-provider-input' } },
      })),
    },
  } }
}

test('frozen model deadline cancels and settles the engine without allowing a replay', async () => {
  const request = fixture()
  request.payload.controlBinding.budgets.modelRequestMs = 10
  let finish
  let aborts = 0
  let settled = false
  const session = controlledSession(async current => {
    await new Promise(resolve => { finish = resolve })
    settled = true
    current.agent.state.messages.push({ role: 'assistant', stopReason: 'stop', content: [] })
  }, async () => { aborts++; finish() })
  await assert.rejects(resumePiKernelBatch(session, request, identity, new AbortController().signal), /deadline exceeded/)
  assert.equal(aborts, 1)
  assert.equal(settled, true)
  await assert.rejects(resumePiKernelBatch(session, request, identity, new AbortController().signal), /already claimed/)
})

test('Host cancellation is observed even when the engine returns an apparently successful response', async () => {
  const host = new AbortController()
  let aborts = 0
  const session = controlledSession(async current => {
    host.abort()
    current.agent.state.messages.push({ role: 'assistant', stopReason: 'stop', content: [] })
  }, async () => { aborts++ })
  await assert.rejects(resumePiKernelBatch(session, fixture(), identity, host.signal), /Host cancelled/)
  assert.equal(aborts, 1)
})

test('an unfinished model response cannot acknowledge a durable batch delivery', async () => {
  for (const final of [null, { role: 'assistant', stopReason: 'error', content: [] },
    { role: 'assistant', stopReason: 'toolUse', content: [] },
    { role: 'assistant', stopReason: 'stop', content: [{ type: 'toolCall', id: 'new', name: 'read', arguments: {} }] }]) {
    const session = controlledSession(async current => { if (final) current.agent.state.messages.push(final) })
    await assert.rejects(resumePiKernelBatch(session, fixture(), identity, new AbortController().signal), /no completed response/)
  }
  const session = controlledSession(async current => {
    current.agent.state.messages.push({ role: 'assistant', stopReason: 'stop', content: [{ type: 'text', text: 'done' }] })
  })
  const result = await resumePiKernelBatch(session, fixture(), identity, new AbortController().signal)
  assert.equal(result.idempotencyKey, 'tool-batch-delivery:batch-1')
})

test('Kernel batch resume requires the complete barrier and preserves source order without repairing facts', () => {
  const request = fixture()
  const before = structuredClone(request)
  const prepared = prepareKernelBatchResume(request, identity)
  assert.deepEqual(prepared.messages.slice(-2).map(message => message.toolCallId), ['read-a', 'read-b'])
  assert.deepEqual(prepared.messages.slice(-2).map(message => message.content[0].text), ['durable result a', 'durable result b'])
  assert.deepEqual(prepared.messages.at(-1).details, {})
  assert.deepEqual(request, before)
  request.payload.batchResume.tools[0].state = 'failed'
  assert.equal(prepareKernelBatchResume(request, identity).messages.at(-1).isError, true)
})

test('Kernel handoff rejects drift, incomplete results and unfinished historical batches', () => {
  const mutations = [
    r => { delete r.payload.controlBinding },
    r => { r.payload.controlBinding.authority = 'legacy' },
    r => { r.payload.controlBinding.engineId = 'codex' },
    r => { r.payload.controlBinding.permissionSnapshotId = 'corrupt' },
    r => { r.runtimeSessionId = 'foreign-session' },
    r => { r.payload.batchResume.checkpointSeq = 0 },
    r => { r.payload.batchResume.idempotencyKey = 'different-key' },
    r => { r.payload.batchResume.tools.pop() },
    r => { r.payload.batchResume.tools[0].toolCallId = 'read-a' },
    r => { r.payload.batchResume.tools[0].canonicalInput.path = 'changed-after-approval' },
    r => { r.payload.batchResume.tools[0].sourceOrder = 0 },
    r => { r.payload.batchResume.tools[0].state = 'running' },
    r => { r.payload.batchResume.tools[0].state = 'cancelled' },
    r => { r.payload.batchResume.tools[0].result.isError = true },
    r => { r.payload.batchResume.tools[0].result = {} },
    r => { r.payload.batchResume.assistantMessage.content[0].arguments.path = 'changed-proposal' },
    r => { r.payload.batchResume.history.push(structuredClone(r.payload.batchResume.assistantMessage)) },
    r => { r.payload.batchResume.history.push({ role: 'toolResult' }) },
    r => { r.payload.batchResume.history[0].content = '中'.repeat(400_000) },
  ]
  for (const mutate of mutations) {
    const request = fixture()
    mutate(request)
    assert.throws(() => prepareKernelBatchResume(request, identity), undefined, mutate.toString())
  }
  assert.throws(() => validatePromptControl(fixture(), 'legacy'), /Legacy adapter/)
})

test('same-session delivery is claimed once and an uncertain continuation is never retried', async () => {
  let starts = 0
  const session = { isIdle: true, state: { isStreaming: false }, abort: async () => {},
    settingsManager: { getRetryEnabled: () => false, getRetrySettings: () => ({ maxRetries: 0 }), getProviderRetrySettings: () => ({ maxRetries: 0 }), getCompactionEnabled: () => false }, agent: {
    state: { messages: [], tools: [] },
    continue: async () => { assert.equal(session.agent.state.messages.at(-1).toolCallId, 'read-b'); starts++; throw new Error('lost after model dispatch') },
  } }
  const signal = new AbortController().signal
  session.agent.state.tools = [{ name: 'read', execute: () => { throw new Error('must not execute') } }]
  await assert.rejects(resumePiKernelBatch(session, fixture(), identity, signal), /executable local tools/)
  session.agent.state.tools = []
  session.settingsManager.getProviderRetrySettings = () => ({ maxRetries: 1 })
  await assert.rejects(resumePiKernelBatch(session, fixture(), identity, signal), /autonomous retry/)
  session.settingsManager.getProviderRetrySettings = () => ({ maxRetries: 0 })
  assert.equal(starts, 0)
  await assert.rejects(resumePiKernelBatch(session, fixture(), identity, signal), /lost after model dispatch/)
  await assert.rejects(resumePiKernelBatch(session, fixture(), identity, signal), /already claimed/)
  assert.equal(starts, 1)
  const cancelled = new AbortController()
  cancelled.abort()
  await assert.rejects(resumePiKernelBatch(session, fixture(), identity, cancelled.signal), /cancelled/)
})

test('a new real Pi process consumes the original batch with durable results instead of reexecuting tools', () => {
  const childPath = fileURLToPath(new URL('./fixtures/pi-kernel-batch-child.mjs', import.meta.url))
  const run = (mode, input) => {
    const child = spawnSync(process.execPath, [childPath, mode], {
      input: JSON.stringify(input), encoding: 'utf8', windowsHide: true, timeout: 20_000, maxBuffer: 2 * 1024 * 1024,
    })
    assert.equal(child.status, 0, `${child.error ?? ''}\n${child.stderr}`)
    return JSON.parse(child.stdout)
  }
  const captured = run('capture', {})
  assert.equal(captured.executions, 0)
  const request = fixture()
  request.payload.batchResume.history = captured.messages.slice(0, -1)
  request.payload.batchResume.assistantMessage = captured.messages.at(-1)
  const resumed = run('resume', { request, identity })
  assert.equal(resumed.executions, 0, 'original tools must not execute in the replacement process')
  assert.deepEqual(resumed.consumed, ['read-a', 'read-b'])
  assert.equal(resumed.answer, 'durable batch consumed')
  assert.equal(resumed.response.assistantMessage.stopReason, 'stop')
  const proposed = run('resume-propose', { request, identity })
  assert.equal(proposed.providerRequests, 1, 'the adapter may not autonomously start the next model request')
  assert.equal(proposed.toolExecutionStarts, 0, 'stop before tool preparation, not only before local execution')
  assert.equal(proposed.executions, 0)
  assert.equal(proposed.response.batchId, request.payload.batchResume.batchId)
  assert.equal(proposed.response.checkpointSeq, request.payload.batchResume.checkpointSeq)
  assert.deepEqual(proposed.response.assistantMessage.content, [
    { type: 'toolCall', id: 'read-c', name: 'read', arguments: { path: 'c.txt' } },
  ])
})
