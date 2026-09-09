import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'
import { createServer } from 'node:http'

const identity = { runId: 'run-k', conversationId: 'conversation-k', runtimeSessionId: 'session-k' }

test('HTTP model handles prior assistant history in initial and tool-batch rounds', { timeout: 30000 }, async t => {
  const requests = []
  const server = createServer(async (req, res) => {
    let body = ''
    for await (const chunk of req) body += chunk
    requests.push(JSON.parse(body))
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    for (const choice of [
      { index: 0, delta: { role: 'assistant', content: '历史续跑成功 😀' }, finish_reason: null },
      { index: 0, delta: {}, finish_reason: 'stop' },
    ]) res.write(`data: ${JSON.stringify({ id: 'local-response', object: 'chat.completion.chunk',
      created: 1, model: 'kernel-http-test', choices: [choice] })}\n\n`)
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })
  for (const mode of ['initial', 'batch']) {
    const child = await worker(t)
    const config = initialization({ modelService: { apiType: 'openai-completions', modelId: 'kernel-http-test',
      baseUrl: `http://127.0.0.1:${server.address().port}/v1`, apiKey: 'local-test-only' } })
    assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
    const payload = mode === 'batch' ? resumePayload() : {
      controlBinding: resumePayload().controlBinding,
      initialModel: { schemaVersion: 1, idempotencyKey: 'initial-model-delivery', checkpointSeq: 2,
        input: { schemaVersion: 1, runId: identity.runId, turnId: 'turn-k', promptConfigHash: 'frozen-hash', messages: [
          { role: 'user', content: 'Earlier question', timestamp: 1 },
          { role: 'assistant', content: 'Earlier answer', timestamp: 2 },
          { role: 'user', content: 'Continue 中文', timestamp: 3 },
        ] } },
    }
    const response = await child.request(mode === 'batch' ? 'kernel.resume_batch' : 'kernel.start_initial', payload)
    assert.equal(response.type, 'kernel.model_response', mode)
    assert.equal(response.payload.response.assistantMessage.stopReason, 'stop')
    assert.equal(response.payload.response.assistantMessage.content.find(block => block.type === 'text').text, '历史续跑成功 😀')
  }
  assert.equal(requests.length, 2)
  assert.ok(requests[0].messages.some(message => message.role === 'assistant' && message.content === 'Earlier answer'))
  assert.ok(requests[1].messages.some(message => message.role === 'tool' && message.tool_call_id === 'read-a'))
})

test('HTTP rejection reports bounded evidence without SDK retry or provider content', { timeout: 30000 }, async t => {
  let calls = 0
  const server = createServer((req,res) => {
    calls += 1
    req.resume()
    res.writeHead(429, { 'content-type': 'application/json', 'retry-after': '2', 'x-secret': 'private' })
    res.end(JSON.stringify({ error: { message: 'private credentials and provider body', type: 'rate_limit_error' } }))
  })
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve))
  t.after(() => { server.closeAllConnections(); server.close() })
  const child = await worker(t)
  const config = initialization({ modelService: { apiType:'openai-completions',modelId:'kernel-http-test',
    baseUrl:`http://127.0.0.1:${server.address().port}/v1`,apiKey:'local-test-only' } })
  assert.equal((await child.request('kernel.initialize',config)).type,'kernel.ready')
  const response = await child.request('kernel.resume_batch',resumePayload())
  assert.equal(response.type,'kernel.model_failure')
  assert.deepEqual(response.payload,{schemaVersion:1,runId:identity.runId,turnId:'turn-k',checkpointSeq:8,
    category:'provider_unavailable',httpStatus:429,retryAfterMs:2000})
  assert.equal(calls,1)
  assert.doesNotMatch(JSON.stringify(response),/private|credentials|local-test-only|x-secret/)
  assert.equal((await child.request('kernel.resume_batch',resumePayload())).type,'request_failed')
  assert.equal(calls,1)
})
test('streamed argument keys and corrective tool feedback survive real HTTP worker rounds exactly', { timeout: 30000 }, async t => {
  const requests = []
  const inputs = [{ objctive: '只回复46', mode: 'worker' }, { objective: '只回复46', mode: 'worker' }]
  const feedback = JSON.stringify({ error: { code: 'child.invalid_arguments', field: 'objective' },
    executionStarted: false, recovery: 'Use the exact key objective in a NEW call.' })
  const server = createServer(async (req, res) => {
    let body = ''
    for await (const chunk of req) body += chunk
    requests.push(JSON.parse(body))
    const round = requests.length - 1
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    const send = delta => res.write(`data: ${JSON.stringify({ id: `stream-${round}`, object: 'chat.completion.chunk',
      created: 1, model: 'kernel-http-test', choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`)
    send({ role: 'assistant', tool_calls: [{ index: 0, id: `child-${round}`, type: 'function',
      function: { name: 'child_run_start', arguments: '' } }] })
    // Split inside both the misspelled and corrected key, not just between JSON values.
    for (const part of JSON.stringify(inputs[round]).match(/.{1,3}/gu)) {
      send({ tool_calls: [{ index: 0, function: { arguments: part } }] })
    }
    res.write(`data: ${JSON.stringify({ id: `stream-${round}`, object: 'chat.completion.chunk', created: 1,
      model: 'kernel-http-test', choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] })}\n\n`)
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })
  const config = initialization({ modelService: { apiType: 'openai-completions', modelId: 'kernel-http-test',
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`, apiKey: 'local-test-only' },
    proposalTools: [{ name: 'child_run_start', description: 'Start a child with objective.', parameters: {
      type: 'object', properties: { objective: { type: 'string' }, mode: { type: 'string' } }, required: ['objective'], additionalProperties: false } }] })
  let previous
  for (let round = 0; round < 2; round++) {
    const child = await worker(t)
    assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
    const payload = resumePayload()
    if (round === 1) {
      payload.batchResume.assistantMessage = previous
      payload.batchResume.tools = [{ toolCallId: 'child-0', tool: 'child_run_start', sourceOrder: 0,
        canonicalInput: inputs[0], state: 'failed', result: { isError: true, content: [{ type: 'text', text: feedback }] } }]
    }
    const response = await child.request('kernel.resume_batch', payload)
    assert.equal(response.type, 'kernel.model_response')
    previous = response.payload.response.assistantMessage
    assert.deepEqual(previous.content.find(block => block.type === 'toolCall').arguments, inputs[round])
    assert.ok(child.events.every(event => event.kind === 'response'))
  }
  assert.equal(requests.length, 2)
  const correctionRequest = requests[1]
  assert.equal(correctionRequest.messages.findLast(message => message.role === 'tool').content, feedback)
  assert.deepEqual(JSON.parse(correctionRequest.messages.findLast(message => message.tool_calls)?.tool_calls[0].function.arguments), inputs[0])
  assert.deepEqual(correctionRequest.tools[0].function.parameters.required, ['objective'])
})

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
      if (message.type === 'kernel.model_preview') continue
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

test('description mode returns scoped schemas and prompt without initializing a model', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const described = await child.request('kernel.describe', {
    executionProfileId: 'legacy',
    modelService: { apiType: 'faux', modelId: 'description-only', baseUrl: 'http://localhost' },
    supportedTools: ['read', 'web_search'],
    prompt: { systemPrompt: 'Host-only instructions', projectContext: { projectRoot: null, permissionMode: 'ask' },
      workSnapshot: { schemaVersion: 1, goal: null, tasks: [], evidence: [] } },
  })
  assert.equal(described.type, 'kernel.description')
  assert.deepEqual(described.payload.proposalTools.map(tool => tool.name), ['web_search'])
  assert.match(described.payload.systemPrompt, /Host-only instructions/)
  assert.match(described.payload.systemPrompt, /FOX_EXECUTION_RECEIPT_V1/)
  assert.match(described.payload.systemPrompt, /bytes, not a character count/)
  assert.match(described.payload.systemPrompt, /Correct the actual JSON argument object in a NEW tool call/)
  assert.match(described.payload.systemPrompt, /not API quota or rate limiting/)
  assert.ok(described.payload.proposalTools.every(tool => Object.keys(tool).sort().join(',') === 'description,name,parameters'))
  assert.equal((await child.request('kernel.initialize', initialization())).type, 'request_failed')
  assert.ok(child.events.every(event => event.kind === 'response'))
})

test('initial model round uses frozen input and does not fabricate a tool batch', { timeout: 30000 }, async t => {
  const child = await worker(t)
  assert.equal((await child.request('kernel.initialize', initialization())).type, 'kernel.ready')
  const payload = { controlBinding: resumePayload().controlBinding, initialModel: {
    schemaVersion: 1, idempotencyKey: 'initial-model-delivery', checkpointSeq: 2,
    input: { schemaVersion: 1, runId: identity.runId, turnId: 'first-turn', promptConfigHash: 'frozen-hash',
      messages: [{ role: 'user', content: 'First question 中文 😀', timestamp: 1 }] } } }
  const invalid = structuredClone(payload); invalid.initialModel.input.messages[0].role = 'system'
  assert.equal((await child.request('kernel.start_initial', invalid)).type, 'request_failed')
  const response = await child.request('kernel.start_initial', payload)
  assert.equal(response.type, 'kernel.model_response')
  assert.equal(response.payload.response.turnId, 'first-turn')
  assert.equal(response.payload.response.batchId, undefined)
  assert.equal(response.payload.response.assistantMessage.stopReason, 'stop')
  assert.equal((await child.request('kernel.start_initial', payload)).type, 'request_failed')
})

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

test('opt-in model previews carry only transient visible text and an ordered delivery cursor', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const config = initialization()
  config.modelService.fauxResponses = [{content:[
    {type:'thinking',thinking:'internal reasoning must not be in display previews'},
    {type:'text',text:'可见的流式回复 😀，最终消息仍须由 Host 提交。'}],stopReason:'stop'}]
  assert.equal((await child.request('kernel.initialize',config)).type,'kernel.ready')
  const payload = resumePayload(); payload.streamPreview = true
  const result = await child.request('kernel.resume_batch',payload)
  assert.equal(result.type,'kernel.model_response')
  const previews = child.events.filter(event=>event.type==='kernel.model_preview').map(event=>event.payload)
  assert.ok(previews.length>0)
  assert.equal(previews.at(-1).text,'可见的流式回复 😀，最终消息仍须由 Host 提交。')
  for (let i=0;i<previews.length;i++) {
    assert.deepEqual(Object.keys(previews[i]).sort(),['checkpointSeq','conversationId','revision','runId','schemaVersion','text','turnId'])
    assert.equal(previews[i].checkpointSeq,8)
    assert.equal(previews[i].runId,identity.runId)
    assert.ok(previews[i].revision>(previews[i-1]?.revision ?? 0))
    assert.doesNotMatch(JSON.stringify(previews[i]),/internal reasoning/)
  }
})
