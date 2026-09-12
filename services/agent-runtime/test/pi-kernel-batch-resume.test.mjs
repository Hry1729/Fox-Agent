import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { prepareKernelModelResponse, prepareKernelBatchResume } from '../src/pi-kernel-batch-resume.mjs'
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
