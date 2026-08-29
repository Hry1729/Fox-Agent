import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { mkdtemp, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'

const runtimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))
const LEGACY_WORKFLOW_MUTATORS = [
  'workflow_start',
  'workflow_stage_start',
  'workflow_stage_complete',
  'workflow_stage_fail',
  'workflow_cancel',
]

function startRuntime() {
  const child = spawn(process.execPath, [runtimePath], { stdio: ['pipe', 'pipe', 'pipe'] })
  const messages = []
  const waiters = []
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    messages.push(JSON.parse(line))
    for (const waiter of [...waiters]) waiter()
  })
  return {
    child,
    send(type, fields = {}) {
      const request = createEnvelope('request', type, fields)
      child.stdin.write(`${JSON.stringify(request)}\n`)
      return request
    },
    async waitFor(predicate, timeout = 4_000) {
      const existing = messages.find(predicate)
      if (existing) return existing
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`timed out: ${JSON.stringify(messages)}`)), timeout)
        const check = () => {
          const message = messages.find(predicate)
          if (!message) return
          clearTimeout(timer)
          resolve(message)
        }
        waiters.push(check)
      })
    },
    close() { output.close(); child.kill() },
  }
}

test('runs the real Pi Agent loop through Fox JSONL with a faux provider', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-runtime-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize', {
    payload: { modelService: { baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 4096, maxOutputTokens: 512 } },
  })
  const ready = await runtime.waitFor((message) => message.requestId === initialize.id)
  assert.equal(ready.type, 'ready')
  assert.equal(ready.payload.runtime, 'fox-pi-runtime')
  for (const toolName of ['task_attempt_start', 'task_repair_start', 'task_attempt_finish']) {
    assert.ok(ready.payload.capabilities.tools.some(({ name }) => name === toolName))
  }
  assert.ok(!ready.payload.capabilities.tools.some(({ name }) => name === 'task_repair_escalate_start'))
  for (const toolName of ['graph_readonly_activate', 'graph_readonly_snapshot_get', 'graph_readonly_node_start', 'graph_readonly_node_review', 'graph_readonly_node_finish', 'graph_readonly_node_cancel', 'graph_readonly_accept']) {
    assert.ok(!ready.payload.capabilities.tools.some(({ name }) => name === toolName))
  }
  assert.ok(ready.payload.capabilities.tools.some(({ name }) => name === 'workflow_snapshot_get'))
  for (const toolName of LEGACY_WORKFLOW_MUTATORS) {
    assert.ok(ready.payload.capabilities.tools.some(({ name }) => name === toolName))
  }

  const sessionId = 'pi-session-1'
  const conversationId = 'conversation-1'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { conversationId, runtimeSessionId: sessionId, payload: { sessionPath } })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId: 'run-1',
    payload: { text: 'hello Pi', messages: [{ role: 'user', content: 'hello Pi' }] },
  })
  await runtime.waitFor((message) => message.runId === 'run-1' && message.payload?.type === 'message.delta')
  await runtime.waitFor((message) => message.runId === 'run-1' && message.payload?.type === 'run.completed')
})

test('records tool and prompt diagnostics while allowing a text-only run with no tools', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-no-tools-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: ['Text-only response.'],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const sessionId = 'pi-no-tools-session'
  const conversationId = 'pi-no-tools-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId: sessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-no-tools-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId,
    payload: {
      text: 'Answer without tools.',
      messages: [{ role: 'user', content: 'Answer without tools.' }],
      assistantPackage: { packageManifest: { allowedTools: [] } },
      expertPackage: { packageManifest: { allowedTools: ['unknown_tool'] } },
      promptBudget: { maxPromptChars: 5_000, charsPerToken: 4 },
    },
  })

  const snapshot = await runtime.waitFor((message) => (
    message.runId === runId && message.payload?.type === 'run.request_snapshot'
  ))
  assert.deepEqual(snapshot.payload.assistantDeclaredToolNames, [])
  assert.deepEqual(snapshot.payload.expertDeclaredToolNames, ['unknown_tool'])
  assert.deepEqual(snapshot.payload.effectiveToolNames, [])
  assert.deepEqual(snapshot.payload.toolNames, [])
  assert.ok(snapshot.payload.excludedTools.some((tool) => tool.reason === 'assistant'))
  assert.deepEqual(
    snapshot.payload.excludedTools.find((tool) => tool.name === 'unknown_tool'),
    { name: 'unknown_tool', reason: 'unregistered', declaredBy: ['expert'] },
  )
  assert.ok(snapshot.payload.promptDiagnostics.totalChars <= snapshot.payload.promptDiagnostics.maxPromptChars)
  assert.equal('prompt' in snapshot.payload.promptDiagnostics, false)
  assert.equal(snapshot.payload.promptDefinitionId, 'fox.runtime.system')
  assert.equal(snapshot.payload.promptVersion, '1.0.0')
  assert.match(snapshot.payload.promptContentHash, /^[a-f0-9]{64}$/)
  assert.match(snapshot.payload.contextSchemaHash, /^[a-f0-9]{64}$/)
  assert.equal(snapshot.payload.promptCacheIdentity.toolCatalogHash, snapshot.payload.toolCatalogHash)
  assert.equal(snapshot.payload.promptCacheDiagnostics.read.eligible, false)
  assert.equal(snapshot.payload.promptCacheDiagnostics.write.eligible, false)

  const answer = await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'message.delta')
  assert.match(answer.payload.delta, /^Text-only/)
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
})

test('validates Execution Profile at startup and snapshots a side-effect-free shadow surface per Run', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-profile-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const invalid = runtime.send('initialize', {
    payload: {
      executionProfile: 'durable_v2',
      executionStrategy: { completionAudit: 'legacy' },
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
      },
    },
  })
  const rejected = await runtime.waitFor((message) => message.requestId === invalid.id)
  assert.equal(rejected.type, 'request_failed')
  assert.match(rejected.payload.message, /Unsupported execution strategy combination/)

  const durableInitialize = runtime.send('initialize', {
    payload: {
      executionProfile: 'durable_v2',
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 32768,
        maxOutputTokens: 512,
      },
    },
  })
  const durableReady = await runtime.waitFor((message) => message.requestId === durableInitialize.id)
  assert.equal(durableReady.type, 'ready')
  assert.equal(durableReady.payload.executionProfile.id, 'durable_v2')
  assert.ok(durableReady.payload.capabilities.tools.some(({ name }) => name === 'workflow_snapshot_get'))
  assert.ok(durableReady.payload.capabilities.tools.every(({ name }) => !LEGACY_WORKFLOW_MUTATORS.includes(name)))
  assert.deepEqual(
    durableReady.payload.capabilities.tools.find(({ name }) => name === 'task_repair_escalate_start'),
    { name: 'task_repair_escalate_start', category: 'work', execution: 'host', approval: 'always' },
  )
  for (const toolName of ['graph_readonly_activate', 'graph_readonly_snapshot_get', 'graph_readonly_node_start', 'graph_readonly_node_review', 'graph_readonly_node_finish', 'graph_readonly_node_cancel', 'graph_readonly_accept']) {
    assert.deepEqual(
      durableReady.payload.capabilities.tools.find(({ name }) => name === toolName),
      { name: toolName, category: 'work', execution: 'host', approval: 'none' },
    )
  }

  const initialize = runtime.send('initialize', {
    payload: {
      executionProfile: 'durable_v2_shadow',
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 32768,
        maxOutputTokens: 512,
        fauxResponses: ['Shadow analysis only.'],
      },
    },
  })
  const ready = await runtime.waitFor((message) => message.requestId === initialize.id)
  assert.equal(ready.type, 'ready')
  assert.equal(ready.payload.executionProfile.id, 'durable_v2_shadow')
  assert.equal(ready.payload.executionProfile.sideEffectsAllowed, false)
  assert.equal(ready.payload.continuationDecisionContract.hostValidationRequired, true)
  assert.ok(ready.payload.capabilities.tools.every((tool) => ![
    'write_file',
    'run_command',
    'web_search',
    'goal_complete',
    'task_attempt_start',
    'task_repair_start',
    'task_repair_escalate_start',
    'task_attempt_finish',
    'graph_readonly_activate',
    'graph_readonly_snapshot_get',
    'graph_readonly_node_start',
    'graph_readonly_node_review',
    'graph_readonly_node_finish',
    'graph_readonly_node_cancel',
    'graph_readonly_accept',
    'child_run_start',
  ].includes(tool.name)))

  const sessionId = 'pi-profile-session'
  const conversationId = 'pi-profile-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId: sessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-profile-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId,
    payload: {
      text: 'Analyze without side effects.',
      messages: [{ role: 'user', content: 'Analyze without side effects.' }],
      assistantPackage: { packageManifest: { allowedTools: ['read'] } },
    },
  })
  const snapshot = await runtime.waitFor((message) => (
    message.runId === runId && message.payload?.type === 'run.request_snapshot'
  ))
  assert.equal(snapshot.payload.executionProfile.id, 'durable_v2_shadow')
  assert.equal(snapshot.payload.executionProfile.shadow, true)
  assert.equal(snapshot.payload.continuationDecisionContract.proposalEventType, 'run.continuation_proposed')
  assert.ok(snapshot.payload.effectiveToolNames.includes('continuation_propose'))
  assert.ok(snapshot.payload.effectiveToolNames.includes('read'))
  assert.ok(!snapshot.payload.effectiveToolNames.includes('write_file'))
  assert.ok(snapshot.payload.excludedTools.some((tool) => tool.name === 'write_file'))
  assert.deepEqual(snapshot.payload.assistantDeclaredToolNames, ['read'])
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')

  const graphInitialize = runtime.send('initialize', {
    payload: {
      executionProfile: 'graph_readonly_preview',
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 32768,
        maxOutputTokens: 512,
      },
    },
  })
  const graphReady = await runtime.waitFor((message) => message.requestId === graphInitialize.id)
  assert.equal(graphReady.type, 'ready')
  assert.equal(graphReady.payload.executionProfile.id, 'graph_readonly_preview')
  assert.deepEqual(
    graphReady.payload.capabilities.tools.find(({ name }) => name === 'graph_readonly_run'),
    { name: 'graph_readonly_run', category: 'project-read', execution: 'runtime', approval: 'none' },
  )
  assert.ok(!durableReady.payload.capabilities.tools.some(({ name }) => name === 'graph_readonly_run'))
  assert.ok(!ready.payload.capabilities.tools.some(({ name }) => name === 'graph_readonly_run'))
  for (const toolName of ['graph_readonly_activate', 'graph_readonly_snapshot_get', 'graph_readonly_node_start', 'graph_readonly_node_review', 'graph_readonly_node_finish', 'graph_readonly_node_cancel', 'graph_readonly_accept']) {
    assert.ok(!graphReady.payload.capabilities.tools.some(({ name }) => name === toolName))
  }

  const reviewerInitialize = runtime.send('initialize', {
    payload: {
      executionProfile: 'graph_reviewer_v1',
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 32768,
        maxOutputTokens: 512,
      },
    },
  })
  const reviewerReady = await runtime.waitFor((message) => message.requestId === reviewerInitialize.id)
  assert.equal(reviewerReady.type, 'ready')
  assert.equal(reviewerReady.payload.executionProfile.id, 'graph_reviewer_v1')
  assert.equal(reviewerReady.payload.executionProfile.strategies.completionAudit, 'strict_v2')
  assert.equal(reviewerReady.payload.executionProfile.strategies.validationPolicy, 'high_risk_v1')
  assert.equal(reviewerReady.payload.executionProfile.strategies.promptPolicy, 'graph_reviewer_v1')
  assert.equal(reviewerReady.payload.executionProfile.continuation.mode, 'disabled')
  assert.equal(reviewerReady.payload.executionProfile.graph.mode, 'disabled')
  assert.equal(reviewerReady.payload.executionProfile.sideEffectsAllowed, false)
  assert.deepEqual(
    reviewerReady.payload.capabilities.tools.map(({ name }) => name),
    ['read', 'ls', 'find', 'grep'],
  )
})

test('keeps a forced Planner session separate from the Executor transcript', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-planner-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        plannerMode: 'always',
        fauxResponses: [
          '{"summary":"Inspect and update","steps":["Inspect files","Apply change","Run tests"],"risks":[],"needsGoal":false}',
          'Executor completed the requested change.',
        ],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const sessionId = 'pi-planner-session'
  const conversationId = 'pi-planner-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId: sessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-planner-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId,
    payload: {
      text: 'Inspect the project, update the implementation, then run tests.',
      messages: [{ role: 'user', content: 'Inspect the project, update the implementation, then run tests.' }],
      projectContext: { projectRoot: directory, permissionMode: 'read_only' },
      workSnapshot: { goal: null, tasks: [], evidence: [] },
    },
  })

  const planned = await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'planner.completed')
  assert.equal(planned.payload.stepCount, 3)
  const answer = await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'message.delta')
  assert.match(answer.payload.delta, /^Executor/)
  assert.doesNotMatch(answer.payload.delta, /Inspect and update/)
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
})

test('maps Anthropic thinking blocks separately from the final answer', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-anthropic-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const server = createServer((request, response) => {
    assert.equal(request.url, '/v1/messages')
    response.writeHead(200, { 'content-type': 'text/event-stream' })
    const events = [
      ['message_start', { type: 'message_start', message: { id: 'msg-1', type: 'message', role: 'assistant', model: 'MiniMax-M3', content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 4, output_tokens: 0 } } }],
      ['content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: '', signature: '' } }],
      ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: '检查任务。' } }],
      ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'signature_delta', signature: 'sig' } }],
      ['content_block_stop', { type: 'content_block_stop', index: 0 }],
      ['content_block_start', { type: 'content_block_start', index: 1, content_block: { type: 'text', text: '' } }],
      ['content_block_delta', { type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: '正式回答。' } }],
      ['content_block_stop', { type: 'content_block_stop', index: 1 }],
      ['message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 6 } }],
      ['message_stop', { type: 'message_stop' }],
    ]
    for (const [name, data] of events) response.write(`event: ${name}\ndata: ${JSON.stringify(data)}\n\n`)
    response.end()
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  context.after(() => server.close())
  const address = server.address()
  assert.ok(address && typeof address === 'object')

  const runtime = startRuntime()
  context.after(() => runtime.close())
  const initialize = runtime.send('initialize', {
    payload: { modelService: { baseUrl: `http://127.0.0.1:${address.port}`, modelId: 'MiniMax-M3', apiType: 'anthropic-messages', apiKey: 'test-key', contextWindow: 4096, maxOutputTokens: 512 } },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  const sessionId = 'pi-anthropic-1'
  const conversationId = 'conversation-anthropic-1'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { conversationId, runtimeSessionId: sessionId, payload: { sessionPath } })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  runtime.send('prompt', { conversationId, runtimeSessionId: sessionId, runId: 'run-anthropic-1', payload: { text: 'test', messages: [{ role: 'user', content: 'test' }] } })
  const reasoning = await runtime.waitFor((message) => message.runId === 'run-anthropic-1' && message.payload?.type === 'reasoning.delta')
  const answer = await runtime.waitFor((message) => message.runId === 'run-anthropic-1' && message.payload?.type === 'message.delta')
  await runtime.waitFor((message) => message.runId === 'run-anthropic-1' && message.payload?.type === 'run.completed')
  assert.equal(reasoning.payload.delta, '检查任务。')
  assert.equal(reasoning.payload.source, 'provider')
  assert.equal(answer.payload.delta, '正式回答。')
})

test('streams OpenAI-compatible reasoning_content separately from the final answer', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-openai-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const server = createServer((request, response) => {
    assert.equal(request.url, '/v1/chat/completions')
    response.writeHead(200, { 'content-type': 'text/event-stream' })
    const chunks = [
      { id: 'chatcmpl-1', object: 'chat.completion.chunk', created: 1, model: 'MiniMax-M3', choices: [{ index: 0, delta: { role: 'assistant', reasoning_content: '检查项目文件。' }, finish_reason: null }] },
      { id: 'chatcmpl-1', object: 'chat.completion.chunk', created: 1, model: 'MiniMax-M3', choices: [{ index: 0, delta: { content: '正式回答。' }, finish_reason: null }] },
      { id: 'chatcmpl-1', object: 'chat.completion.chunk', created: 1, model: 'MiniMax-M3', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 4, completion_tokens: 6, total_tokens: 10 } },
    ]
    for (const chunk of chunks) response.write(`data: ${JSON.stringify(chunk)}\n\n`)
    response.write('data: [DONE]\n\n')
    response.end()
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  context.after(() => server.close())
  const address = server.address()
  assert.ok(address && typeof address === 'object')

  const runtime = startRuntime()
  context.after(() => runtime.close())
  const initialize = runtime.send('initialize', {
    payload: { modelService: { baseUrl: `http://127.0.0.1:${address.port}/v1`, modelId: 'MiniMax-M3', apiType: 'openai-completions', apiKey: 'test-key', contextWindow: 4096, maxOutputTokens: 512 } },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  const sessionId = 'pi-openai-1'
  const conversationId = 'conversation-openai-1'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { conversationId, runtimeSessionId: sessionId, payload: { sessionPath } })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  runtime.send('prompt', { conversationId, runtimeSessionId: sessionId, runId: 'run-openai-1', payload: { text: 'test', messages: [{ role: 'user', content: 'test' }] } })
  const reasoning = await runtime.waitFor((message) => message.runId === 'run-openai-1' && message.payload?.type === 'reasoning.delta')
  const answer = await runtime.waitFor((message) => message.runId === 'run-openai-1' && message.payload?.type === 'message.delta')
  await runtime.waitFor((message) => message.runId === 'run-openai-1' && message.payload?.type === 'run.completed')
  assert.equal(reasoning.payload.delta, '检查项目文件。')
  assert.equal(reasoning.payload.source, 'provider')
  assert.equal(reasoning.payload.providerField, 'reasoning_content')
  assert.equal(answer.payload.delta, '正式回答。')
})
