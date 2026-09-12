import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { KERNEL_COMPLETION_CONTRACT, completionRequired } from '../src/kernel-completion.mjs'

const identity = { runId: 'run-k', conversationId: 'conversation-k', runtimeSessionId: 'session-k' }

test('HTTP final-answer contract rejects premature stops and keeps explicit answers, blockers and tools', { timeout: 60000 }, async t => {
  let plan
  const requests = []
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk
    const body = JSON.parse(raw); requests.push(body)
    const step = plan.responses[Math.min(requests.length - 1, plan.responses.length - 1)]
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    const text = step.text ?? plan.text
    const deltas = step.tool ? [{ role: 'assistant', tool_calls: [{ index: 0, id: 'next-read', type: 'function',
      function: { name: 'read', arguments: '{"path":"a.txt"}' } }] }]
      : [{ role: 'assistant', reasoning_content: 'Provider thinking' }, ...(text ?? '').split('').map(content => ({ content }))]
    for (const delta of deltas) res.write(`data: ${JSON.stringify({ id: 'completion-test', object: 'chat.completion.chunk',
      created: 1, model: 'kernel-http-test', choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`)
    res.write(`data: ${JSON.stringify({ id: 'completion-test', object: 'chat.completion.chunk', created: 1, model: 'kernel-http-test',
      choices: [{ index: 0, delta: {}, finish_reason: step.tool ? 'tool_calls' : 'stop' }] })}\n\n`)
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })
  const onRoundOutput = frame => {
    if (frame.assistantMessage.stopReason === 'toolUse') {
      return { schemaVersion: 1, kind: 'batch', batchId: 'host-batch-1', checkpointSeq: 9, tools: [
        { toolCallId: 'next-read', tool: 'read', sourceOrder: 0, canonicalInput: { path: 'a.txt' },
          state: 'completed', result: { content: [{ type: 'text', text: 'host file body' }] } }] }
    }
    return { schemaVersion: 1, kind: 'final' }
  }
  const cases = [
    { text: 'Let me thoroughly analyze the data from the spreadsheet to produce accurate statistics for the report.', responses: [{}], incomplete: true },
    { text: '', responses: [{}], incomplete: true },
    { text: '42\n<fox-final/>\n', responses: [{}], answer: '42' },
    { text: '请上传原始表格。\n<fox-final/>', responses: [{}], answer: '请上传原始表格。' },
    { responses: [{ tool: true }, { text: '已完成统计。\n<fox-final/>' }], answer: '已完成统计。', toolRound: true },
  ]
  for (const mode of ['initial', 'batch']) for (const item of cases) {
    plan = item
    requests.length = 0
    const child = await worker(t, ['--kernel-worker'], { onRoundOutput })
    await child.request('kernel.initialize', initialization({ systemPrompt: KERNEL_COMPLETION_CONTRACT,
      modelService: { apiType: 'openai-completions', modelId: 'kernel-http-test', apiKey: 'local-test-only',
        baseUrl: `http://127.0.0.1:${server.address().port}/v1` } }))
    const payload = mode === 'batch' ? resumePayload() : { controlBinding: resumePayload().controlBinding,
      initialModel: { schemaVersion: 1, idempotencyKey: 'initial-model-delivery', checkpointSeq: 2,
        input: { schemaVersion: 1, runId: identity.runId, turnId: 'turn-k', promptConfigHash: 'frozen-hash',
          messages: [{ role: 'user', content: '完成表格分析和报告；缺少资料时说明。', timestamp: 1 }] } } }
    payload.streamPreview = true
    const command = mode === 'batch' ? 'kernel.resume_batch' : 'kernel.start_initial'
    const result = await child.request(command, payload)
    assert.equal(result.type, item.incomplete ? 'kernel.model_failure' : 'kernel.model_response')
    assert.equal(requests.length, item.responses.length, 'the engine loop only calls the model for planned rounds')
    assert.ok(requests[0].messages.some(message => typeof message.content === 'string' && message.content.includes('Fox final-answer protocol v1')))
    if (item.incomplete) {
      assert.equal(result.payload.category, 'incomplete_response')
      assert.equal(child.roundOutputs.length, 0, 'an incomplete stop never reaches the Host boundary')
    } else if (item.toolRound) {
      assert.equal(child.roundOutputs[0].assistantMessage.stopReason, 'toolUse')
      assert.deepEqual(child.roundOutputs[0].assistantMessage.content.find(block => block.type === 'toolCall').arguments, { path: 'a.txt' })
      assert.equal(result.payload.response.assistantMessage.content.filter(block => block.type === 'text').map(block => block.text).join(''), item.answer)
      assert.ok(child.events.filter(event => event.type === 'kernel.model_preview').every(event => !event.payload.text.includes('<fox-')))
    } else {
      assert.equal(result.payload.response.assistantMessage.content.filter(block => block.type === 'text').map(block => block.text).join(''), item.answer)
      assert.ok(child.events.filter(event => event.type === 'kernel.model_preview').every(event => !event.payload.text.includes('<fox-')))
    }
    assert.equal((await child.request(command, payload)).type, 'request_failed')
  }
})

for (const engine of ['deepseek_harness', 'codex']) {
  test(`isolated ${engine} worker supports native continuation and Host compaction`,
    { skip: engine === 'codex' && !process.env.FOX_TEST_CODEX_BINARY, timeout: 90000 }, async t => {
      let calls = 0
      const server = createServer(async (req, res) => {
        let raw = ''; for await (const chunk of req) raw += chunk
        const body = JSON.parse(raw); calls++
        assert.equal(body.model, engine === 'codex' ? 'gpt-5.1-codex' : 'deepseek-v4-flash')
        if (calls === 2) assert.ok(!body.tools?.length)
        res.writeHead(200, { 'content-type': 'text/event-stream' })
        if (engine === 'codex') {
          const item = { type: 'message', id: `msg-${calls}`, role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: 'Native continuation 中文', annotations: [] }] }
          for (const event of [{ type: 'response.created', response: { id: `resp-${calls}` } },
            { type: 'response.output_item.done', output_index: 0, item },
            { type: 'response.completed', response: { id: `resp-${calls}`, status: 'completed', output: [item], usage: { input_tokens: 20, output_tokens: 5, total_tokens: 25 } } }]) res.write(`data: ${JSON.stringify(event)}\n\n`)
          res.end()
        } else {
          for (const choice of [{ index: 0, delta: { role: 'assistant', content: 'Native continuation 中文' }, finish_reason: null },
            { index: 0, delta: {}, finish_reason: 'stop' }]) res.write(`data: ${JSON.stringify({ id: `resp-${calls}`, object: 'chat.completion.chunk', model: body.model, created: 1, choices: [choice] })}\n\n`)
          res.end('data: [DONE]\n\n')
        }
      })
      await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
      t.after(() => { server.closeAllConnections(); server.close() })
      const nativeAdapter = engine === 'codex' ? { command: process.env.FOX_TEST_CODEX_BINARY,
        binaryHash: `sha256:${createHash('sha256').update(await readFile(process.env.FOX_TEST_CODEX_BINARY)).digest('hex')}` } : undefined
      for (const mode of ['batch', 'compaction']) {
        const child = await worker(t)
        assert.equal((await child.request('kernel.initialize', initialization({ engineId: engine, nativeAdapter,
          proposalTools: mode === 'compaction' ? [] : initialization().proposalTools,
          modelService: { apiType: engine === 'codex' ? 'openai-responses' : 'openai-completions', modelId: engine === 'codex' ? 'gpt-5.1-codex' : 'deepseek-v4-flash',
            baseUrl: `http://127.0.0.1:${server.address().port}/v1`, apiKey: 'local-test', reasoning: false } }))).type, 'kernel.ready')
        const payload = mode === 'batch' ? resumePayload() : compactionPayload()
        payload.controlBinding.engineId = engine
        const result = await child.request(mode === 'batch' ? 'kernel.resume_batch' : 'kernel.compact_context', payload)
        assert.equal(result.type, mode === 'batch' ? 'kernel.model_response' : 'kernel.compaction_result')
        assert.ok(JSON.stringify(result).includes('Native continuation 中文'))
      }
      assert.equal(calls, 2)
    })
}

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
    if (round >= 2) {
      send({ role: 'assistant', content: '46' })
      send({})
    } else {
      send({ role: 'assistant', tool_calls: [{ index: 0, id: `child-${round}`, type: 'function',
        function: { name: 'child_run_start', arguments: '' } }] })
      // Split inside both the misspelled and corrected key, not just between JSON values.
      for (const part of JSON.stringify(inputs[round]).match(/.{1,3}/gu)) {
        send({ tool_calls: [{ index: 0, function: { arguments: part } }] })
      }
    }
    res.write(`data: ${JSON.stringify({ id: `stream-${round}`, object: 'chat.completion.chunk', created: 1,
      model: 'kernel-http-test', choices: [{ index: 0, delta: {}, finish_reason: round >= 2 ? 'stop' : 'tool_calls' }] })}\n\n`)
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })
  const config = initialization({ modelService: { apiType: 'openai-completions', modelId: 'kernel-http-test',
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`, apiKey: 'local-test-only' },
    proposalTools: [{ name: 'child_run_start', description: 'Start a child with objective.', parameters: {
      type: 'object', properties: { objective: { type: 'string' }, mode: { type: 'string' } }, required: ['objective'], additionalProperties: false } }] })
  const onRoundOutput = frame => {
    if (frame.assistantMessage.stopReason !== 'toolUse') return { schemaVersion: 1, kind: 'final' }
    const round = child.roundOutputs.length - 1
    return { schemaVersion: 1, kind: 'batch', batchId: `host-batch-${round}`, checkpointSeq: 20 + round, tools: [
      { toolCallId: `child-${round}`, tool: 'child_run_start', sourceOrder: 0, canonicalInput: inputs[round],
        state: round === 0 ? 'failed' : 'completed',
        result: { content: [{ type: 'text', text: round === 0 ? feedback : 'child completed with 46' }] } }] }
  }
  const child = await worker(t, ['--kernel-worker'], { onRoundOutput })
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const response = await child.request('kernel.resume_batch', resumePayload())
  assert.equal(response.type, 'kernel.model_response')
  assert.equal(child.roundOutputs.length, 3, 'two tool proposals and one final answer cross the Host boundary')
  assert.deepEqual(child.roundOutputs[0].assistantMessage.content.find(block => block.type === 'toolCall').arguments, inputs[0])
  assert.deepEqual(child.roundOutputs[1].assistantMessage.content.find(block => block.type === 'toolCall').arguments, inputs[1])
  assert.equal(child.roundOutputs[2].assistantMessage.stopReason, 'stop')
  assert.equal(requests.length, 3)
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

function compactionPayload() {
  return {controlBinding:resumePayload().controlBinding,compaction:{schemaVersion:1,runId:identity.runId,
    turnId:'turn-k',compactionId:'summary-k',inputHash:`sha256:${'a'.repeat(64)}`,
    messages:[{role:'user',content:'Earlier request: retain constraints and uncertainty.'}],maxSummaryBytes:1024}}
}

test('Host cancellation aborts an active compaction and prevents any late result or replay', {timeout:30000}, async t=>{
  const child=await worker(t)
  await child.request('kernel.initialize',initialization({proposalTools:[],modelService:{apiType:'faux',modelId:'slow-summary',
    baseUrl:'http://localhost',fauxTokensPerSecond:1,fauxResponses:['a deliberately slow summary that must be cancelled']}}))
  const pending=child.request('kernel.compact_context',compactionPayload())
  await new Promise(resolve=>setTimeout(resolve,150))
  const started=performance.now()
  assert.equal((await child.request('kernel.cancel')).type,'kernel.cancelling')
  assert.equal((await pending).type,'request_failed')
  assert.ok(performance.now()-started<5000)
  assert.ok(child.events.every(event=>event.type!=='kernel.compaction_result'))
  assert.equal((await child.request('kernel.compact_context',compactionPayload())).type,'request_failed')
})

test('Host compaction uses a real single-use HTTP model with no tools and a distinct result', {timeout:30000}, async t=>{
  const requests=[]
  const server=createServer(async(req,res)=>{
    let body='';for await(const chunk of req) body+=chunk
    requests.push(JSON.parse(body))
    res.writeHead(200,{'content-type':'text/event-stream'})
    for(const choice of [{index:0,delta:{role:'assistant',content:'保留约束；尚未验证执行结果。'},finish_reason:null},
      {index:0,delta:{},finish_reason:'stop'}]) res.write(`data: ${JSON.stringify({id:'summary',object:'chat.completion.chunk',created:1,model:'summary-test',choices:[choice]})}\n\n`)
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve))
  t.after(()=>{server.closeAllConnections();server.close()})
  const child=await worker(t)
  const ready=await child.request('kernel.initialize',initialization({proposalTools:[],systemPrompt:'Summarize only; no task execution.',
    modelService:{apiType:'openai-completions',modelId:'summary-test',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,apiKey:'local-test'}}))
  assert.equal(ready.payload.hostCompaction,true)
  const result=await child.request('kernel.compact_context',compactionPayload())
  assert.equal(result.type,'kernel.compaction_result')
  assert.equal(result.payload.summary,'保留约束；尚未验证执行结果。')
  assert.equal(result.payload.compactionId,'summary-k')
  assert.equal(result.payload.inputHash,compactionPayload().compaction.inputHash)
  assert.equal(requests.length,1)
  assert.ok(!requests[0].tools?.length)
  assert.equal((await child.request('kernel.compact_context',compactionPayload())).type,'request_failed')
  assert.equal((await child.request('kernel.start_initial',{})).type,'request_failed')
  assert.ok(child.events.every(event=>event.kind==='response' && event.type!=='kernel.model_preview'))
})

test('compaction rejects retained tool proposals, overlong summaries and model tool calls', {timeout:30000}, async t=>{
  const withTools=await worker(t)
  await withTools.request('kernel.initialize',initialization())
  assert.equal((await withTools.request('kernel.compact_context',compactionPayload())).type,'request_failed')
  for(const response of ['中'.repeat(600),{stopReason:'toolUse',content:[{type:'toolCall',id:'bad',name:'read',arguments:{path:'never.txt'}}]}]) {
    const child=await worker(t)
    await child.request('kernel.initialize',initialization({proposalTools:[],modelService:{apiType:'faux',modelId:'summary-test',
      baseUrl:'http://localhost',fauxResponses:[response]}}))
    assert.equal((await child.request('kernel.compact_context',compactionPayload())).type,'request_failed')
    assert.ok(child.events.every(event=>event.kind==='response'))
  }
})
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

async function worker(t, args = ['--kernel-worker'], { onRoundOutput, mapHostResponse = reply => reply } = {}) {
  const binary = process.env.FOX_KERNEL_WORKER_BINARY
  const child = spawn(binary ?? process.execPath, binary ? args : [fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url)), ...args],
    { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
  const pending = new Map()
  const events = []
  const roundOutputs = []
  let output = ''
  let stderr = ''
  const answerRoundOutput = onRoundOutput ?? (frame => ({ schemaVersion: 1, kind: 'final' }))
  child.stderr.on('data', chunk => { stderr += chunk })
  child.stdout.on('data', chunk => {
    output += chunk
    let end
    while ((end = output.indexOf('\n')) >= 0) {
      const line = output.slice(0, end); output = output.slice(end + 1)
      const message = JSON.parse(line)
      events.push(message)
      if (message.kind === 'request' && message.type === 'kernel.round_output') {
        roundOutputs.push(message.payload)
        const directive = answerRoundOutput(message.payload)
        // Returning null simulates the Host rejecting the boundary (e.g. it
        // observed cancellation before committing the terminal decision).
        const reply = directive === null
          ? createEnvelope('response', 'request_failed',
              { requestId: message.id, runId: message.runId, conversationId: message.conversationId,
                runtimeSessionId: message.runtimeSessionId, payload: { code: 'kernel.host_rejected', message: 'Host rejected the round boundary' } })
          : createEnvelope('response', 'kernel.round_directive',
              { requestId: message.id, runId: message.runId, conversationId: message.conversationId,
                runtimeSessionId: message.runtimeSessionId, payload: directive })
        child.stdin.write(`${JSON.stringify(mapHostResponse(reply))}\n`)
        continue
      }
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
    // A Host that services round boundaries drives the live loop; other
    // deliveries use the single-round transport (execution omitted => once).
    if (type === 'kernel.initialize' && onRoundOutput && payload.execution === undefined) {
      payload.execution = 'loop'
    }
    const message = createEnvelope('request', type, { ...identity, payload, ...fields })
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { pending.delete(message.id); reject(new Error(`No response for ${type}: ${stderr}`)) }, 20000)
      pending.set(message.id, result => { clearTimeout(timeout); resolve(result) })
      child.stdin.write(`${JSON.stringify(message)}\n`)
    })
  }
  return { request, events, roundOutputs }
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
  // New Kernel prompts no longer force a final-answer marker; the Host drives
  // a bounded stop review instead. Old frozen prompts keep their contract.
  assert.equal(completionRequired(described.payload.systemPrompt), false)
  assert.ok(described.payload.proposalTools.every(tool => Object.keys(tool).sort().join(',') === 'description,name,parameters'))
  assert.equal((await child.request('kernel.initialize', initialization())).type, 'request_failed')
  assert.ok(child.events.every(event => event.kind === 'response' || (event.kind === 'request' && event.type === 'kernel.round_output')))
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
  assert.ok(child.events.every(event => event.kind === 'response' || (event.kind === 'request' && event.type === 'kernel.round_output')))
})

test('real isolated worker hands tool proposals to the Host and executes nothing locally', { timeout: 30000 }, async t => {
  const child = await worker(t, ['--kernel-worker'], { onRoundOutput: frame => {
    if (frame.assistantMessage.stopReason === 'toolUse') {
      return { schemaVersion: 1, kind: 'batch', batchId: 'host-batch-1', checkpointSeq: 9, tools: [
        { toolCallId: 'next-read', tool: 'read', sourceOrder: 0, canonicalInput: { path: 'must-not-open.txt' },
          state: 'completed', result: { content: [{ type: 'text', text: 'Host-settled body only' }] } }] }
    }
    return { schemaVersion: 1, kind: 'final' }
  } })
  const config = initialization()
  config.modelService.fauxResponses = [
    { content: [{ type: 'toolCall', id: 'next-read', name: 'read', arguments: { path: 'must-not-open.txt' } }], stopReason: 'toolUse' },
    { content: [{ type: 'text', text: 'I reported the Host result only.' }], stopReason: 'stop' },
  ]
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const result = await child.request('kernel.resume_batch', resumePayload())
  assert.equal(result.type, 'kernel.model_response')
  assert.equal(result.payload.response.assistantMessage.stopReason, 'stop')
  assert.equal(child.roundOutputs[0].assistantMessage.content[0].arguments.path, 'must-not-open.txt')
  assert.equal(child.roundOutputs.length, 2)
  assert.ok(child.events.every(event => event.kind === 'response' || event.type === 'kernel.round_output'))
})

test('a Host rejection of the final round boundary wins over an apparent success', { timeout: 30000 }, async t => {
  // The model produces an apparently complete answer, but the Host refuses the
  // terminal commit (it observed cancellation/closed world before settling).
  // The worker must surface a consumed failure, never a late success, and the
  // durable decision stays with the Host.
  const child = await worker(t, ['--kernel-worker'], { onRoundOutput: () => null })
  const config = initialization()
  config.modelService.fauxResponses = [{ content: [{ type: 'text', text: 'apparent final answer' }], stopReason: 'stop' }]
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const result = await child.request('kernel.resume_batch', resumePayload())
  assert.equal(result.type, 'request_failed')
  assert.match(result.payload.message, /reconcile/i)
  assert.equal(child.roundOutputs.length, 1)
  // The process is consumed; no hidden retry can deliver the answer.
  assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'request_failed')
})

test('a malformed Host directive fails the live round closed without local execution', { timeout: 30000 }, async t => {
  const child = await worker(t, ['--kernel-worker'], { onRoundOutput: frame =>
    frame.assistantMessage.stopReason === 'toolUse'
      ? { schemaVersion: 1, kind: 'batch', batchId: 'b1', checkpointSeq: 9, tools: [
        { toolCallId: 'next-read', tool: 'read', sourceOrder: 0, canonicalInput: { path: 'a.txt' },
          state: 'completed', result: { content: [{ type: 'text', text: 'settled' }] } }] }
      : { schemaVersion: 1, kind: 'unexpected' } })
  const config = initialization()
  config.modelService.fauxResponses = [
    { content: [{ type: 'toolCall', id: 'next-read', name: 'read', arguments: { path: 'a.txt' } }], stopReason: 'toolUse' },
    { content: [{ type: 'text', text: 'done' }], stopReason: 'stop' },
  ]
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const result = await child.request('kernel.resume_batch', resumePayload())
  assert.equal(result.type, 'request_failed')
  assert.match(result.payload.message, /reconcile|rejected/)
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

test('live Host replies require the pending request and full envelope identity', { timeout: 30000 }, async t => {
  for (const [key, value] of [
    ['conversationId', null], ['runId', 'foreign-run'], ['runtimeSessionId', 'foreign-session'],
    ['requestId', 'foreign-request'], ['protocol', 'foreign-protocol'], ['version', 99],
    ['type', 'kernel.ready'], ['id', undefined], ['timestamp', undefined],
  ]) await t.test(key, async sub => {
    const child = await worker(sub, ['--kernel-worker'], {
      onRoundOutput: () => ({ schemaVersion: 1, kind: 'final' }),
      mapHostResponse: reply => ({ ...reply, [key]: value }),
    })
    assert.equal((await child.request('kernel.initialize', initialization())).type, 'kernel.ready')
    const result = await child.request('kernel.resume_batch', resumePayload())
    assert.equal(result.type, 'request_failed')
    assert.equal(child.roundOutputs.length, 1, 'a foreign reply cannot admit another round')
    assert.equal((await child.request('kernel.resume_batch', resumePayload())).type, 'request_failed')
  })
})

test('continuation previews and final acknowledgement share the Host cursor', { timeout: 30000 }, async t => {
  let round = 0
  const child = await worker(t, ['--kernel-worker'], { onRoundOutput: () => ++round === 1
    ? { schemaVersion: 1, kind: 'continuation', prompt: 'Check remaining work.', previewSeq: 17 }
    : { schemaVersion: 1, kind: 'final' } })
  const config = initialization()
  config.modelService.fauxResponses = ['I will begin.', 'The final answer.']
  await child.request('kernel.initialize', config)
  const payload = resumePayload(); payload.streamPreview = true
  const result = await child.request('kernel.resume_batch', payload)
  assert.equal(result.type, 'kernel.model_response')
  const previews = child.events.filter(event => event.type === 'kernel.model_preview')
  assert.equal(previews.at(-1).payload.checkpointSeq, 17)
  assert.equal(result.payload.response.checkpointSeq, 17)
  assert.equal(result.payload.response.assistantMessage.content[0].text, 'The final answer.')
  assert.ok(previews.every((event, index) => index === 0 || event.payload.revision > previews[index - 1].payload.revision))
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

test('live round deadline bounds the wait before the first response header', { timeout: 30000 }, async t => {
  // The server accepts the request but withholds response headers (and thus
  // the first assistant streaming event) far beyond the model round budget.
  // The deadline is armed before the provider request is issued, so the round
  // must fail inside the budget even though streaming never started.
  let release
  const held = new Promise(resolve => { release = resolve })
  const server = createServer(async (req, res) => {
    for await (const _ of req) {}
    await held
    if (res.writableEnded || res.destroyed) return
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { release(); server.closeAllConnections?.(); server.close() })
  const child = await worker(t, ['--kernel-worker'], { onRoundOutput: () => ({ schemaVersion: 1, kind: 'final' }) })
  const config = initialization({
    modelService: { apiType: 'openai-completions', modelId: 'slow-heads', baseUrl: `http://127.0.0.1:${server.address().port}/v1`, apiKey: 'k' },
  })
  assert.equal((await child.request('kernel.initialize', config)).type, 'kernel.ready')
  const payload = { controlBinding: { ...resumePayload().controlBinding, budgets: { ...resumePayload().controlBinding.budgets, modelRequestMs: 100 } } }
  const start = performance.now()
  const response = await child.request('kernel.start_initial', {
    controlBinding: payload.controlBinding,
    initialModel: { schemaVersion: 1, idempotencyKey: 'initial-model-delivery', checkpointSeq: 2,
      input: { schemaVersion: 1, runId: identity.runId, turnId: 'slow-turn', promptConfigHash: 'h',
        messages: [{ role: 'user', content: 'go', timestamp: 1 }] } },
  })
  const elapsed = performance.now() - start
  assert.equal(response.type, 'request_failed')
  assert.match(response.payload.message, /reconcile/)
  assert.ok(elapsed < 1000, `deadline must fire near the 100ms budget, took ${elapsed}ms`)
  assert.equal(child.roundOutputs.length, 0, 'no round output is committed when headers never arrive')
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

test('opt-in model previews keep provider reasoning separate from answer text with an ordered cursor', { timeout: 30000 }, async t => {
  const child = await worker(t)
  const config = initialization()
  config.modelService.fauxResponses = [{content:[
    {type:'thinking',thinking:'供应商提供的思考说明'},
    {type:'text',text:'可见的流式回复 😀，最终消息仍须由 Host 提交。'}],stopReason:'stop'}]
  assert.equal((await child.request('kernel.initialize',config)).type,'kernel.ready')
  const payload = resumePayload(); payload.streamPreview = true
  const result = await child.request('kernel.resume_batch',payload)
  assert.equal(result.type,'kernel.model_response')
  const previews = child.events.filter(event=>event.type==='kernel.model_preview').map(event=>event.payload)
  assert.ok(previews.length>0)
  assert.equal(previews.at(-1).text,'可见的流式回复 😀，最终消息仍须由 Host 提交。')
  assert.equal(previews.at(-1).reasoning,'供应商提供的思考说明')
  for (let i=0;i<previews.length;i++) {
    assert.ok(Object.keys(previews[i]).every(key=>['checkpointSeq','conversationId','reasoning','revision','runId','schemaVersion','text','turnId'].includes(key)))
    assert.equal(previews[i].checkpointSeq,8)
    assert.equal(previews[i].runId,identity.runId)
    assert.ok(previews[i].revision>(previews[i-1]?.revision ?? 0))
    assert.doesNotMatch(previews[i].text,/供应商提供的思考说明/)
  }
})
