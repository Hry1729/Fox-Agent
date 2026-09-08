import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'

const identity = { runId: 'run-k', conversationId: 'conversation-k', runtimeSessionId: 'session-k' }
function initialization(overrides = {}) {
  return { executionProfileId: 'legacy', systemPrompt: 'Use only the supplied durable history.',
    proposalTools: [{ name: 'read', description: 'Propose a Host file read.', parameters: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } }],
    modelService: { apiType: 'faux', modelId: 'kernel-test', baseUrl: 'http://localhost', fauxResponses: ['durable batch consumed'] },
    ...overrides }
}
function resumePayload() {
  const permission = { mode: 'read_only', projectRoot: null, grants: [] }
  return { controlBinding: { schemaVersion: 1, runId: identity.runId, conversationId: identity.conversationId,
    engineId: 'pi', executionProfileId: 'legacy', authority: 'authoritative', readOnlyExecutor: 'rust', permission,
    permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`,
    budgets: { modelRequestMs: 120000, toolExecutionMs: 600000, runExecutionMs: 1800000, approvalWaitMs: 300000 } },
    batchResume: { schemaVersion: 1, turnId: 'turn-k', batchId: 'batch-k', idempotencyKey: 'tool-batch-delivery:batch-k', checkpointSeq: 8,
      history: [{ role: 'user', content: 'Read the file', timestamp: 1 }],
      assistantMessage: { role: 'assistant', stopReason: 'toolUse', timestamp: 2,
        content: [{ type: 'toolCall', id: 'read-a', name: 'read', arguments: { path: 'a.txt' } }] },
      tools: [{ toolCallId: 'read-a', tool: 'read', sourceOrder: 0, canonicalInput: { path: 'a.txt' },
        state: 'completed', result: { content: [{ type: 'text', text: 'persisted result' }] } }] } }
}

async function worker(t, args = ['--kernel-worker']) {
  const binary = process.env.FOX_KERNEL_WORKER_BINARY
  const child = spawn(binary ?? process.execPath, binary ? args : [fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url)), ...args],
    { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
  const pending = new Map()
  const events = []
  let output = ''
  let stderr = ''
  child.stderr.on('data', chunk => { stderr += chunk })
  child.stdout.on('data', chunk => {
    output += chunk
    let end
    while ((end = output.indexOf('\n')) >= 0) {
      const line = output.slice(0, end); output = output.slice(end + 1)
      const message = JSON.parse(line)
      events.push(message)
      pending.get(message.requestId)?.(message)
      pending.delete(message.requestId)
    }
  })
  const exited = new Promise(resolve => child.once('exit', resolve))
  child.on('error', error => { stderr += error.message })
  t.after(async () => {
    child.stdin.end()
    child.kill()
    await exited
    assert.equal(stderr, '')
  })
  function request(type, payload = {}, fields = {}) {
    const message = createEnvelope('request', type, { ...identity, payload, ...fields })
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { pending.delete(message.id); reject(new Error(`No response for ${type}: ${stderr}`)) }, 20000)
      pending.set(message.id, result => { clearTimeout(timeout); resolve(result) })
      child.stdin.write(`${JSON.stringify(message)}\n`)
    })
  }
  return { request, events }
}

test('real isolated worker returns one model response and refuses replay or Legacy commands', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const ready = await child.request('kernel.initialize', initialization())
  assert.equal(ready.type, 'kernel.ready')
  assert.equal(ready.payload.adapterVersion, 'pi-0.84.2/fox-kernel-worker-v1')
  assert.equal((await child.request('prompt', {})).type, 'request_failed')
  assert.equal((await child.request('kernel.initialize', initialization())).type, 'request_failed')
  const result = await child.request('kernel.resume_batch', resumePayload())
  assert.equal(result.type, 'kernel.model_response')
  assert.equal(result.payload.response.assistantMessage.content[0].text, 'durable batch consumed')
  assert.equal(result.payload.response.checkpointSeq, 8)
  assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'request_failed')
  assert.ok(child.events.every(event => event.kind === 'response'))
})

test('real isolated worker proposes tools without executing them', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const config = initialization()
  config.modelService.fauxResponses = [{ content: [{ type: 'toolCall', id: 'next-read', name: 'read', arguments: { path: 'must-not-open.txt' } }], stopReason: 'toolUse' }]
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const result = await child.request('kernel.resume_batch', resumePayload())
  assert.equal(result.type, 'kernel.model_response')
  assert.equal(result.payload.response.assistantMessage.stopReason, 'toolUse')
  assert.equal(result.payload.response.assistantMessage.content[0].arguments.path, 'must-not-open.txt')
  assert.ok(child.events.every(event => event.kind === 'response'))
})

test('wrong identity, authority and incomplete results cannot dispatch', { timeout: 30000 }, async t => {
  const child = await worker(t)
  assert.equal((await child.request('kernel.initialize', initialization())).type, 'kernel.ready')
  assert.equal((await child.request('kernel.resume_batch', resumePayload(), { runId: 'other' })).type, 'request_failed')
  assert.equal((await child.request('kernel.cancel', {}, { conversationId: 'other' })).type, 'request_failed')
  const legacy = resumePayload(); legacy.controlBinding.authority = 'legacy'
  assert.equal((await child.request('kernel.resume_batch', legacy)).type, 'request_failed')
  const incomplete = resumePayload(); incomplete.batchResume.tools = []
  assert.equal((await child.request('kernel.resume_batch', incomplete)).type, 'request_failed')
  assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'kernel.model_response')
})

test('cancellation settles the model request and permanently consumes the process', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const config = initialization(); config.modelService.fauxTokensPerSecond = 1
  config.modelService.fauxResponses = ['This response takes long enough to cancel before completion.']
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const response = child.request('kernel.resume_batch', resumePayload())
  assert.equal((await child.request('kernel.cancel')).type, 'kernel.cancelling')
  assert.equal((await response).type, 'request_failed')
  assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'request_failed')
})

test('ordinary Legacy process rejects the isolated Kernel protocol', { timeout: 30000 }, async t => {
  const child = await worker(t, [])
  assert.equal((await child.request('kernel.initialize', initialization())).type, 'request_failed')
  assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'request_failed')
})

test('frozen model timeout consumes the worker without leaking input into diagnostics', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const config = initialization(); config.modelService.fauxTokensPerSecond = 1
  config.modelService.apiKey = 'private-test-key-not-for-logs'
  config.modelService.fauxResponses = ['Private provider response that must never become an error diagnostic.']
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const payload = resumePayload(); payload.controlBinding.budgets.modelRequestMs = 10
  const response = await child.request('kernel.resume_batch', payload)
  assert.equal(response.type, 'request_failed')
  assert.match(response.payload.message, /reconcile/)
  assert.doesNotMatch(JSON.stringify(child.events), /private-test-key|Private provider/)
  assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'request_failed')
})

test('concurrent initialization is single-owner and cancellation prevents readiness', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const initializing = child.request('kernel.initialize', initialization())
  const duplicate = child.request('kernel.initialize', initialization())
  const cancelled = child.request('kernel.cancel')
  assert.equal((await duplicate).type, 'request_failed')
  assert.equal((await cancelled).type, 'kernel.cancelling')
  assert.equal((await initializing).type, 'request_failed')
  assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'request_failed')
  assert.ok(child.events.every(event => event.type !== 'kernel.ready'))
})
