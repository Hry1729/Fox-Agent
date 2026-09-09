import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { createHash } from 'node:crypto'
import { runDeepSeekKernelModel, deepSeekHistory } from '../src/deepseek-kernel-adapter.mjs'
import { createKernelWorker } from '../src/pi-kernel-worker.mjs'
import { createEnvelope } from '../src/protocol.mjs'

async function endpoint(t, response) {
  const requests = []
  const server = createServer(async (req, res) => {
    let body = ''; for await (const chunk of req) body += chunk
    requests.push(JSON.parse(body)); response(req, res)
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })
  return { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, requests }
}
function stream(res, tool = false) {
  res.writeHead(200, { 'content-type': 'text/event-stream' })
  const delta = tool ? { tool_calls: [{ index: 0, id: 'read-native', type: 'function', function: { name: 'read', arguments: '{"path":"proof.txt"}' } }] } : { content: '已核验 😀' }
  for (const choice of [{ index: 0, delta: { role: 'assistant', ...delta }, finish_reason: null }, { index: 0, delta: {}, finish_reason: tool ? 'tool_calls' : 'stop' }]) {
    res.write(`data: ${JSON.stringify({ id: 'native-dsh', object: 'chat.completion.chunk', model: 'deepseek-v4-flash', created: 1, choices: [choice] })}\n\n`)
  }
  res.end('data: [DONE]\n\n')
}
const configuration = baseUrl => ({ engineId: 'deepseek_harness', executionProfileId: 'legacy',
  modelService: { apiType: 'openai-completions', baseUrl, modelId: 'deepseek-v4-flash', maxOutputTokens: 512, reasoning: false },
  systemPrompt: 'Follow Host facts.', proposalTools: [{ name: 'read', description: 'Read via Host', parameters: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } }],
})
const history = () => [
  { role: 'user', content: 'Original request' },
  { role: 'assistant', content: [{ type: 'toolCall', id: 'old-call', name: 'read', arguments: { path: 'proof.txt' } }] },
  { role: 'toolResult', toolCallId: 'old-call', toolName: 'read', isError: true, content: [{ type: 'text', text: 'FOX_EXECUTION_RECEIPT_V1: confirmed error' }] },
]

test('real Harness SDK keeps checkpoints and returns proposals without any tool executor', async t => {
  const service = await endpoint(t, (_, res) => stream(res, true))
  const result = await runDeepSeekKernelModel({ config: configuration(service.baseUrl), messages: history(), apiKey: 'isolated' })
  assert.equal(result.stopReason, 'toolUse')
  assert.deepEqual(result.content.find(b => b.type === 'toolCall'), { type: 'toolCall', id: 'read-native', name: 'read', arguments: { path: 'proof.txt' } })
  assert.equal(service.requests.length, 1)
  assert.equal(service.requests[0].max_tokens, 512)
  assert.ok(service.requests[0].messages.some(m => m.role === 'tool' && m.tool_call_id === 'old-call' && m.content.includes('FOX_EXECUTION_RECEIPT_V1')))
  assert.deepEqual(service.requests[0].tools.map(t => t.function.name), ['read'])
})

test('Harness failures and cancellation never automatically retry', async t => {
  const service = await endpoint(t, (_, res) => { res.writeHead(429); res.end('{"error":{"message":"private provider diagnostic"}}') })
  await assert.rejects(runDeepSeekKernelModel({ config: configuration(service.baseUrl), messages: [{ role: 'user', content: 'hello' }], apiKey: 'isolated' }))
  assert.equal(service.requests.length, 1)
  const abort = new AbortController()
  const slow = await endpoint(t, () => abort.abort())
  await assert.rejects(runDeepSeekKernelModel({ config: configuration(slow.baseUrl), messages: [{ role: 'user', content: 'hello' }], apiKey: 'isolated', signal: abort.signal }))
  assert.equal(slow.requests.length, 1)
})

test('Harness adapter does not silently omit unsupported image history', () => {
  assert.throws(() => deepSeekHistory([{ role: 'user', content: [{ type: 'image', mimeType: 'image/png', data: 'eA==' }] }], 'deepseek-v4-flash'))
})

test('shared worker binds Harness identity, returns wire response and rejects repeated delivery', async t => {
  const service = await endpoint(t, (_, res) => stream(res))
  const output = []; const worker = createKernelWorker(frame => output.push(frame))
  t.after(() => worker.close())
  const identity = { runId: 'native-run', conversationId: 'native-conversation', runtimeSessionId: 'native-session' }
  const request = (type, payload) => createEnvelope('request', type, { ...identity, payload })
  const config = configuration(service.baseUrl); config.modelService.apiKey = 'isolated'
  await worker.handle(request('kernel.initialize', config))
  assert.equal(output.pop().type, 'kernel.ready')
  const permission = { mode: 'read_only', projectRoot: null, grants: [] }
  const binding = { schemaVersion: 1, ...identity, engineId: 'deepseek_harness', executionProfileId: 'legacy', authority: 'authoritative', readOnlyExecutor: 'rust',
    permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`, permission,
    budgets: { modelRequestMs: 30000, toolExecutionMs: 30000, runExecutionMs: 60000, approvalWaitMs: 60000 } }
  delete binding.runtimeSessionId
  const payload = { controlBinding: binding, initialModel: { schemaVersion: 1, idempotencyKey: 'initial-model-delivery', checkpointSeq: 2,
    input: { schemaVersion: 1, runId: identity.runId, turnId: 'turn', promptConfigHash: 'frozen', messages: [{ role: 'user', content: 'hello' }] } } }
  await worker.handle(request('kernel.start_initial', payload))
  const response = output.pop()
  assert.equal(response.type, 'kernel.model_response')
  assert.equal(response.payload.idempotencyKey, 'initial-model-delivery')
  assert.equal(response.payload.response.assistantMessage.content.find(b => b.type === 'text').text, '已核验 😀')
  await worker.handle(request('kernel.start_initial', payload))
  assert.equal(output.pop().type, 'request_failed')
  assert.equal(service.requests.length, 1)
})
