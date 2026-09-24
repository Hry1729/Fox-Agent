import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { createServer } from 'node:http'
import { fileURLToPath } from 'node:url'
import { canonicalPermission } from '../src/control-binding.mjs'
import { createEnvelope } from '../src/protocol.mjs'

const identity = { runId: 'notice-run', conversationId: 'notice-conversation', runtimeSessionId: 'notice-session' }
const marker = 'FOX_HOST_JOB_NOTICE_V1\n'

function fact(jobId) {
  return { source: 'fox_kernel_host', dataRootId: `sha256:${'a'.repeat(64)}`,
    conversationId: identity.conversationId, runId: identity.runId, jobId, attempt: 1,
    terminalState: 'completed', finishedAt: 42, resultRef: `fox-result://${identity.runId}/${jobId}`,
    resultSha256: `sha256:${'b'.repeat(64)}`, resultBytes: 5, errorCode: null }
}

function binding() {
  const permission = { mode: 'read_only', projectRoot: null, grants: [] }
  return { schemaVersion: 1, runId: identity.runId, conversationId: identity.conversationId,
    engineId: 'pi', executionProfileId: 'legacy', authority: 'authoritative', readOnlyExecutor: 'rust', permission,
    permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(canonicalPermission(permission))).digest('hex')}`,
    budgets: { modelRequestMs: 120000, toolExecutionMs: 600000, runExecutionMs: 1800000, approvalWaitMs: 300000 } }
}

function initial(notices = []) {
  return { controlBinding: binding(), initialModel: { schemaVersion: 1,
    idempotencyKey: 'initial-model-delivery', checkpointSeq: 2, hostJobNotices: notices,
    input: { schemaVersion: 1, runId: identity.runId, turnId: 'notice-turn', promptConfigHash: 'frozen-hash',
      messages: [{ role: 'user', content: 'Continue from Host facts.', timestamp: 1 }] } } }
}

function batch(historical, notices) {
  return { controlBinding: binding(), batchResume: { schemaVersion: 1, turnId: 'notice-turn',
    batchId: 'notice-batch', idempotencyKey: 'tool-batch-delivery:notice-batch', checkpointSeq: 8,
    history: [{ role: 'user', content: 'Read once.', timestamp: 1 },
      { role: 'hostJobNotice', notice: historical }],
    assistantMessage: { role: 'assistant', stopReason: 'toolUse', timestamp: 2,
      content: [{ type: 'toolCall', id: 'read-a', name: 'read', arguments: { path: 'a.txt' } }] },
    tools: [{ toolCallId: 'read-a', tool: 'read', sourceOrder: 0, canonicalInput: { path: 'a.txt' },
      state: 'completed', result: { content: [{ type: 'text', text: 'Host-settled file body' }] } }],
    hostJobNotices: notices } }
}

function config(execution) {
  return { executionProfileId: 'legacy', execution, systemPrompt: 'Use only Host-settled facts.',
    proposalTools: [{ name: 'read', description: 'Propose a Host file read.',
      parameters: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } }],
    modelService: { apiType: 'openai-completions', modelId: 'notice-http-test',
      baseUrl: '', apiKey: 'local-test-only' } }
}

function messageText(message) {
  return typeof message.content === 'string' ? message.content
    : Array.isArray(message.content) ? message.content.map(block => block.text ?? '').join('') : ''
}

function noticeMessages(request) {
  return request.messages.filter(message => message.role === 'user' && messageText(message).startsWith(marker))
}

async function provider(t, responses) {
  const requests = []
  const server = createServer(async (req, res) => {
    let raw = ''
    for await (const chunk of req) raw += chunk
    const body = JSON.parse(raw)
    requests.push(body)
    const step = responses[requests.length - 1]
    assert.ok(step, 'unexpected extra Provider request')
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    const send = (delta, finishReason = null) => res.write(`data: ${JSON.stringify({
      id: `notice-${requests.length}`, object: 'chat.completion.chunk', created: 1,
      model: 'notice-http-test', choices: [{ index: 0, delta, finish_reason: finishReason }],
    })}\n\n`)
    if (step.tool) {
      send({ role: 'assistant', tool_calls: [{ index: 0, id: 'read-a', type: 'function',
        function: { name: 'read', arguments: '{"path":"a.txt"}' } }] })
      send({}, 'tool_calls')
    } else {
      send({ role: 'assistant', content: step.text })
      send({}, 'stop')
    }
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })
  return { requests, baseUrl: `http://127.0.0.1:${server.address().port}/v1` }
}

async function worker(t, onRoundOutput) {
  const child = spawn(process.execPath,
    [fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url)), '--kernel-worker'],
    { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
  const pending = new Map()
  const rounds = []
  let stdout = ''
  let stderr = ''
  let transportError
  child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-4096) })
  child.on('error', error => { transportError = error })
  child.stdout.on('data', chunk => {
    stdout += chunk
    let end
    while ((end = stdout.indexOf('\n')) >= 0) {
      const line = stdout.slice(0, end); stdout = stdout.slice(end + 1)
      let message
      try { message = JSON.parse(line) } catch (error) { transportError = error; continue }
      if (message.kind === 'request' && message.type === 'kernel.round_output') {
        rounds.push(message.payload)
        const directive = onRoundOutput?.(message.payload) ?? { schemaVersion: 1, kind: 'final' }
        const reply = createEnvelope('response', 'kernel.round_directive', {
          requestId: message.id, runId: message.runId, conversationId: message.conversationId,
          runtimeSessionId: message.runtimeSessionId, payload: directive })
        child.stdin.write(`${JSON.stringify(reply)}\n`)
        continue
      }
      if (message.type === 'kernel.model_preview' || message.type === 'kernel.usage_record') continue
      pending.get(message.requestId)?.(message)
      pending.delete(message.requestId)
    }
  })
  const exited = new Promise(resolve => child.once('close', resolve))
  t.after(async () => {
    child.stdin.end()
    child.kill()
    let closeTimeout
    try {
      await Promise.race([exited, new Promise((_, reject) => {
        closeTimeout = setTimeout(() => reject(new Error('worker did not close')), 2000)
      })])
    } finally {
      clearTimeout(closeTimeout)
    }
    assert.equal(transportError, undefined)
    assert.equal(stderr, '')
  })
  function request(type, payload, timeoutMs = type === 'kernel.initialize' ? 12000 : 10000) {
    const message = createEnvelope('request', type, { ...identity, payload })
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { pending.delete(message.id); reject(new Error(`${type} timed out: ${stderr}`)) }, timeoutMs)
      pending.set(message.id, result => { clearTimeout(timeout); resolve(result) })
      child.stdin.write(`${JSON.stringify(message)}\n`)
    })
  }
  return { request, rounds }
}

test('real live worker projects a new Host notice once after a settled tool round', { timeout: 45000 }, async t => {
  const http = await provider(t, [{ tool: true }, { text: 'Review done.' }, { text: 'Finished.' }])
  const notice = fact('compute-job')
  let round = 0
  const child = await worker(t, frame => {
    round++
    if (round === 1) {
      assert.equal(frame.assistantMessage.stopReason, 'toolUse')
      return { schemaVersion: 1, kind: 'batch', batchId: 'notice-batch', checkpointSeq: 9,
        tools: [{ toolCallId: 'read-a', tool: 'read', sourceOrder: 0, canonicalInput: { path: 'a.txt' },
          state: 'completed', result: { content: [{ type: 'text', text: 'Host-settled file body' }] } }],
        hostJobNotices: [notice] }
    }
    if (round === 2) return { schemaVersion: 1, kind: 'continuation', prompt: 'Give the final answer.', previewSeq: 11 }
    return { schemaVersion: 1, kind: 'final' }
  })
  const setup = config('loop'); setup.modelService.baseUrl = http.baseUrl
  assert.equal((await child.request('kernel.initialize', setup)).type, 'kernel.ready')
  const payload = initial()
  const result = await child.request('kernel.start_initial', payload)
  assert.equal(result.type, 'kernel.model_response', JSON.stringify(result))
  assert.equal(child.rounds.length, 3)
  assert.equal(http.requests.length, 3)
  assert.deepEqual(http.requests.map(request => noticeMessages(request).length), [0, 1, 1])
  for (const request of http.requests.slice(1)) {
    assert.match(messageText(noticeMessages(request)[0]), /"jobId":"compute-job"/)
    assert.equal(request.messages.filter(message => message.role === 'tool' && message.tool_call_id === 'read-a').length, 1,
      'the notice is not a second formal result for the original toolCallId')
  }
  assert.equal(payload.initialModel.hostJobNotices.length, 0)
})

test('real per-round workers project typed historical and new facts to Provider without adding a tool result',
  { timeout: 45000 }, async t => {
    const http = await provider(t, [{ text: 'Initial fact seen.' }, { text: 'Batch facts seen.' }])
    const historical = fact('already-delivered')
    const next = fact('newly-delivered')
    for (const [type, payload] of [
      ['kernel.start_initial', initial([historical])],
      ['kernel.resume_batch', batch(historical, [next])],
    ]) {
      const child = await worker(t)
      const setup = config('once'); setup.modelService.baseUrl = http.baseUrl
      assert.equal((await child.request('kernel.initialize', setup)).type, 'kernel.ready')
      const snapshot = structuredClone(payload)
      const result = await child.request(type, payload)
      assert.equal(result.type, 'kernel.model_response', JSON.stringify(result))
      assert.deepEqual(payload, snapshot, 'the worker must not upgrade Host markers in the supplied frame')
    }
    assert.equal(http.requests.length, 2)
    assert.equal(noticeMessages(http.requests[0]).length, 1)
    assert.match(messageText(noticeMessages(http.requests[0])[0]), /"jobId":"already-delivered"/)
    assert.equal(noticeMessages(http.requests[1]).length, 2)
    assert.deepEqual(noticeMessages(http.requests[1]).map(message =>
      JSON.parse(messageText(message).split('\n').at(-1)).jobId), ['already-delivered', 'newly-delivered'])
    assert.equal(http.requests[1].messages.filter(message => message.role === 'tool' && message.tool_call_id === 'read-a').length, 1)
    assert.equal(http.requests[1].messages.filter(message => message.role === 'tool' && messageText(message).includes(marker)).length, 0)
  })

test('worker entry rejects fake, duplicate and oversized notice DTOs before Provider I/O', { timeout: 30000 }, async t => {
  const http = await provider(t, [{ text: 'Valid fact seen.' }])
  const child = await worker(t)
  const setup = config('once'); setup.modelService.baseUrl = http.baseUrl
  assert.equal((await child.request('kernel.initialize', setup)).type, 'kernel.ready')
  const invalid = [
    [{ ...fact('fake-source'), source: 'user' }],
    [fact('duplicate'), fact('duplicate')],
    [{ ...fact('oversized'), jobId: '中'.repeat(700) }],
    Array.from({ length: 10 }, (_, index) => ({ ...fact(`many-${index}`),
      resultRef: `fox-result://${'r'.repeat(1400)}-${index}` })),
  ]
  for (const notices of invalid) {
    const rejected = await child.request('kernel.start_initial', initial(notices))
    assert.equal(rejected.type, 'request_failed')
    assert.equal(http.requests.length, 0, 'invalid DTO must not reach the Provider')
  }
  assert.equal((await child.request('kernel.start_initial', initial([fact('valid')]))).type, 'kernel.model_response')
  assert.equal(http.requests.length, 1)
  assert.equal(noticeMessages(http.requests[0]).length, 1)
})
