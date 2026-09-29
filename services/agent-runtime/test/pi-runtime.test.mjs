import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
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
  const childClosed = new Promise((resolve) => child.once('close', resolve))
  // A spawn error still emits close, but must not become an unhandled event.
  child.on('error', () => {})
  const messages = []
  const waiters = []
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    messages.push(JSON.parse(line))
    for (const waiter of [...waiters]) waiter()
  })
  return {
    child,
    messages,
    send(type, fields = {}) {
      const request = createEnvelope('request', type, fields)
      child.stdin.write(`${JSON.stringify(request)}\n`)
      return request
    },
    respond(request, type, payload = {}) {
      const response = createEnvelope('response', type, {
        requestId: request.id,
        conversationId: request.conversationId,
        runtimeSessionId: request.runtimeSessionId,
        runId: request.runId,
        payload,
      })
      child.stdin.write(`${JSON.stringify(response)}\n`)
      return response
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
    async close() {
      output.close()
      if (child.exitCode === null && child.signalCode === null) child.kill()
      let timer
      try {
        await Promise.race([
          childClosed,
          new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error('Pi runtime did not close within 5 seconds')), 5_000)
          }),
        ])
      } finally {
        clearTimeout(timer)
      }
    },
  }
}

async function closeRuntimeAndRemoveDirectory(runtime, directory, remove = rm) {
  try {
    await runtime?.close()
  } finally {
    await remove(directory, { recursive: true, force: true })
  }
}

function registerRuntimeCleanup(context, runtime, directory) {
  context.after(() => closeRuntimeAndRemoveDirectory(runtime, directory))
}

test('runtime cleanup closes the child even when directory removal fails', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-cleanup-failure-'))
  const runtime = startRuntime()
  try {
    await assert.rejects(
      closeRuntimeAndRemoveDirectory(runtime, directory, async () => {
        assert.ok(runtime.child.exitCode !== null || runtime.child.signalCode !== null,
          'directory removal must start only after the real child exits')
        throw new Error('forced directory removal failure')
      }),
      /forced directory removal failure/,
    )
    assert.ok(runtime.child.exitCode !== null || runtime.child.signalCode !== null)
  } finally {
    await runtime.close()
    await rm(directory, { recursive: true, force: true })
  }
})

test('runs the real Pi Agent loop through Fox JSONL with a faux provider', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-runtime-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

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

test('unknown Office name stays an SDK error and recovers through the declared MCP wrapper', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-office-routing-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 16384, maxOutputTokens: 512,
    fauxResponses: [
      { content: [{ type: 'toolCall', id: 'bad-name', name: 'office_help', arguments: { format: 'xlsx' } }], stopReason: 'toolUse' },
      { content: [{ type: 'toolCall', id: 'catalog', name: 'list_mcp_tools', arguments: {} }], stopReason: 'toolUse' },
      { content: [{ type: 'toolCall', id: 'wrapped', name: 'call_mcp_tool', arguments: { serverId: 'fox-office', tool: 'office_help', arguments: { format: 'xlsx' } } }], stopReason: 'toolUse' },
      '已查询 Office 帮助。',
    ],
  } } })
  const ready = await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const declared = new Set(ready.payload.capabilities.tools.map(tool => tool.name))
  const identity = { conversationId: 'office-c', runtimeSessionId: 'office-s' }
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { ...identity, payload: { sessionPath } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  const runId = 'office-recovery'
  runtime.send('prompt', { ...identity, runId, payload: { text: '查询 Excel 帮助', projectContext: { projectRoot: null } } })
  for (const name of ['list_mcp_tools', 'call_mcp_tool']) {
    const call = await runtime.waitFor(message => message.type === 'tool.execute' && message.runId === runId && message.payload?.tool === name)
    if (name === 'call_mcp_tool') assert.equal(call.payload.input.tool, 'office_help')
    runtime.respond(call, 'tool.execute_completed', { isError: false, result: { content: [{ type: 'text', text: 'Office catalog/help' }] } })
  }
  await runtime.waitFor(message => message.runId === runId && message.payload?.type === 'run.completed')
  const events = runtime.messages.filter(message => message.runId === runId).map(message => message.payload)
  assert.ok(events.some(event => event?.phase === 'tool.rejected' && event.tool === 'office_help'))
  for (const event of events.filter(event => ['tool.started', 'tool.updated', 'tool.completed'].includes(event?.type))) assert.ok(declared.has(event.tool), event.tool)
  assert.ok(!runtime.messages.some(message => message.type === 'tool.execute' && message.payload?.tool === 'office_help'))
  const session = JSON.parse(await readFile(sessionPath, 'utf8'))
  const rejected = session.messages.find(message => message.role === 'toolResult' && message.toolName === 'office_help')
  assert.equal(rejected.isError, true)
  assert.match(rejected.content[0].text, /not found/)
})

for (const referenceForm of ['object-array', 'serialized-object']) {
test(`projectless knowledge question reaches Host search and preserves evidence (${referenceForm})`, async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-knowledge-routing-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const reference = { source: 'remote', connectionId: 'yuxi-primary', id: 'kb-crane' }
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 16384, maxOutputTokens: 512,
    fauxResponses: [
      { content: [{ type: 'toolCall', id: 'kb-list', name: 'list_knowledge_bases', arguments: {} }], stopReason: 'toolUse' },
      { content: [{ type: 'toolCall', id: 'kb-search', name: 'search_knowledge', arguments: { ...(referenceForm === 'object-array' ? { targets: [reference] } : { target: JSON.stringify(reference) }), query: '轨道吊 起升 赋值' } }], stopReason: 'toolUse' },
      '已找到知识库资料。',
    ],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identity = { conversationId: 'kb-conversation', runtimeSessionId: 'kb-session' }
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { ...identity, payload: { sessionPath } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  const runId = 'kb-run'
  runtime.send('prompt', { ...identity, runId, payload: { text: '如何给轨道吊起升赋值', projectContext: { projectRoot: null } } })
  const snapshot = await runtime.waitFor(message => message.runId === runId && message.payload?.type === 'run.request_snapshot')
  for (const name of ['read', 'ls', 'find', 'grep', 'run_command']) {
    assert.ok(!snapshot.payload.effectiveToolNames.includes(name))
    assert.ok(snapshot.payload.excludedTools.some(tool => tool.name === name && tool.reason === 'project_unavailable'))
  }
  assert.ok(snapshot.payload.effectiveToolNames.includes('search_knowledge'))
  const list = await runtime.waitFor(message => message.type === 'tool.execute' && message.payload?.tool === 'list_knowledge_bases')
  runtime.respond(list, 'tool.execute_completed', { isError: false, result: { items: [{ reference, available: true }] } })
  const search = await runtime.waitFor(message => message.type === 'tool.execute' && message.payload?.tool === 'search_knowledge')
  assert.deepEqual(search.payload.input.target ?? search.payload.input.targets[0], reference)
  const started = await runtime.waitFor(message => message.runId === runId
    && message.payload?.type === 'tool.started' && message.payload?.toolCallId === search.payload.toolCallId)
  assert.deepEqual(started.payload.input, search.payload.input,
    'persisted tool identity must equal the normalized Host execution input')
  const evidence = { items: [{ documentId: 'crane-manual', content: '回归测试资料片段', source: 'remote' }] }
  runtime.respond(search, 'tool.execute_completed', { isError: false, result: evidence })
  await runtime.waitFor(message => message.runId === runId && message.payload?.type === 'run.completed')
  assert.ok(!runtime.messages.some(message => message.type === 'tool.preflight'))
  const session = JSON.parse(await readFile(sessionPath, 'utf8'))
  const result = session.messages.find(message => message.role === 'toolResult' && message.toolName === 'search_knowledge')
  assert.deepEqual(JSON.parse(result.content[0].text), evidence)
})
}

test('keeps every structured Host result visible when one model turn calls multiple tools', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-parallel-results-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: [
          {
            content: [
              { type: 'toolCall', id: 'child-list-call', name: 'child_agent_list', arguments: {} },
              { type: 'toolCall', id: 'workflow-call', name: 'workflow_snapshot_get', arguments: {} },
            ],
            stopReason: 'toolUse',
          },
          'Both structured tool results were received.',
        ],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const runtimeSessionId = 'pi-parallel-results-session'
  const conversationId = 'pi-parallel-results-conversation'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-parallel-results-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'List child agents and inspect the workflow in one turn.',
      messages: [{ role: 'user', content: 'List child agents and inspect the workflow in one turn.' }],
    },
  })

  const childListRequest = await runtime.waitFor((message) => (
    message.kind === 'request'
    && message.type === 'tool.execute'
    && message.runId === runId
    && message.payload?.tool === 'child_agent_list'
  ))
  const workflowRequest = await runtime.waitFor((message) => (
    message.kind === 'request'
    && message.type === 'tool.execute'
    && message.runId === runId
    && message.payload?.tool === 'workflow_snapshot_get'
  ))
  runtime.respond(childListRequest, 'tool.execute_completed', {
    isError: false,
    result: { agents: [{ id: 'fox-general' }, { id: 'fox-reviewer' }] },
  })
  runtime.respond(workflowRequest, 'tool.execute_completed', {
    isError: false,
    result: { workflow: { id: 'workflow-1', status: 'running' } },
  })

  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  const session = JSON.parse(await readFile(sessionPath, 'utf8'))
  const results = Object.fromEntries(session.messages
    .filter((message) => message.role === 'toolResult')
    .map((message) => [message.toolName, message]))

  assert.deepEqual(JSON.parse(results.child_agent_list.content[0].text), {
    agents: [{ id: 'fox-general' }, { id: 'fox-reviewer' }],
  })
  assert.deepEqual(JSON.parse(results.workflow_snapshot_get.content[0].text), {
    workflow: { id: 'workflow-1', status: 'running' },
  })
})

test('applies bounded correction on consecutive no-progress calls and never re-executes them', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-tool-budget-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  // The model proposes the identical search every turn. With the legacy
  // maxIdenticalToolCalls:1 override (now the no-progress streak limit), the
  // first repeat still executes once (its outcome could have changed); every
  // later proposal is refused without executing, and only a model that keeps
  // ignoring the correction stops the run.
  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: Array.from({ length: 6 }, (_, index) => ({
          content: [{ type: 'toolCall', id: `stuck-${index + 1}`, name: 'search_knowledge', arguments: { query: 'same query' } }],
          stopReason: 'toolUse',
        })),
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const runtimeSessionId = 'pi-tool-budget-session'
  const conversationId = 'pi-tool-budget-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-tool-budget-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'Keep searching the same thing.',
      messages: [{ role: 'user', content: 'Keep searching the same thing.' }],
      runBudget: { maxIdenticalToolCalls: 1 },
    },
  })

  // The initial call and its first repeat both execute (the repeat's outcome
  // could legitimately have changed); after it returns the identical result,
  // every later proposal must be refused without a Host execution.
  for (const callId of ['stuck-1', 'stuck-2']) {
    const execute = await runtime.waitFor((message) => message.kind === 'request'
      && message.type === 'tool.execute' && message.runId === runId && message.payload?.toolCallId === callId)
    runtime.respond(execute, 'tool.execute_completed', {
      isError: false,
      result: { items: [{ documentId: 'doc-1', content: 'same evidence' }] },
    })
  }

  const failure = await runtime.waitFor((message) => (
    message.runId === runId && message.payload?.type === 'run.failed'
  ))
  assert.equal(failure.payload.code, 'runtime.no_progress')
  // Corrections refuse re-execution: no repeated side effects or quota spend
  // after the first repeat settled with an identical result.
  const executions = runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'tool.execute' && message.runId === runId && message.payload?.tool === 'search_knowledge')
  assert.equal(executions.length, 2)
  // The bounded correction surfaced as a tool-level error with guidance
  // before the run stopped, not as an immediate run failure.
  const session = JSON.parse(await readFile(join(directory, 'session.json'), 'utf8'))
  const correction = session.messages.find((message) => message.role === 'toolResult'
    && message.toolName === 'search_knowledge' && message.content?.[0]?.text?.includes('有界纠偏'))
  assert.ok(correction, 'a tool-level no-progress correction is recorded')
  assert.ok(correction.content[0].text.includes('没有新进展'))
  await new Promise((resolve) => setTimeout(resolve, 50))
  assert.equal(runtime.messages.some((message) => (
    message.runId === runId && message.payload?.type === 'run.completed'
  )), false)
})

test('a 7th productive web search is allowed; only no-evidence repeats are corrected', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-web-search-budget-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: [
          {
            content: Array.from({ length: 7 }, (_, index) => ({
              type: 'toolCall',
              id: `web-search-${index + 1}`,
              name: 'web_search',
              arguments: { query: `distinct query ${index + 1}` },
            })),
            stopReason: 'toolUse',
          },
          'Seven distinct searches completed and the evidence is enough.',
        ],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const runtimeSessionId = 'pi-web-search-budget-session'
  const conversationId = 'pi-web-search-budget-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-web-search-budget-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'Research this thoroughly.',
      messages: [{ role: 'user', content: 'Research this thoroughly.' }],
    },
  })

  // There is no fixed per-run search count anymore: all seven distinct
  // queries execute and the run completes normally.
  for (let index = 1; index <= 7; index += 1) {
    const request = await runtime.waitFor((message) => message.kind === 'request'
      && message.type === 'tool.execute' && message.runId === runId
      && message.payload?.toolCallId === `web-search-${index}`)
    runtime.respond(request, 'tool.execute_completed', {
      isError: false,
      result: { results: [{ title: `evidence ${index}`, url: `https://example.com/${index}` }] },
    })
  }
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  assert.equal(runtime.messages.some((message) => (
    message.runId === runId && message.payload?.type === 'run.failed'
  )), false)
})

test('polling with changing results and read-after-write continue past the old fixed count', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-polling-progress-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  // Same input every turn, but the result keeps changing (state advanced,
  // e.g. a file rewritten between reads): every call is real progress and
  // must execute, well beyond the old cross-run count of 4.
  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: [
          ...Array.from({ length: 5 }, (_, index) => ({
            content: [{ type: 'toolCall', id: `poll-${index + 1}`, name: 'search_knowledge', arguments: { query: 'status' } }],
            stopReason: 'toolUse',
          })),
          'State kept changing; polling is done.',
        ],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const runtimeSessionId = 'pi-polling-session'
  const conversationId = 'pi-polling-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-polling-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'Watch the status until it settles.',
      messages: [{ role: 'user', content: 'Watch the status until it settles.' }],
    },
  })

  for (let index = 1; index <= 5; index += 1) {
    const request = await runtime.waitFor((message) => message.kind === 'request'
      && message.type === 'tool.execute' && message.runId === runId
      && message.payload?.toolCallId === `poll-${index}`)
    runtime.respond(request, 'tool.execute_completed', {
      isError: false,
      result: { items: [{ documentId: 'doc-1', content: `state revision ${index}` }] },
    })
  }
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  const executions = runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'tool.execute' && message.runId === runId && message.payload?.tool === 'search_knowledge')
  assert.equal(executions.length, 5)
})

test('real progress from a wait-semantic tool disarms the no-progress ladder for re-reads', async (context) => {
  // ABC R6 reproduction: identical reads reach the streak bound, then a
  // productive wait-semantic tool (code_check whose outcome changed), then the
  // same read again. The productive result must clear the streak and the
  // correction ladder so the next read executes and observes the new state.
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-progress-after-wait-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: [
          ...Array.from({ length: 4 }, (_, index) => ({
            content: [{ type: 'toolCall', id: `read-${index + 1}`, name: 'search_knowledge', arguments: { query: 'file' } }],
            stopReason: 'toolUse',
          })),
          { content: [{ type: 'toolCall', id: 'wait-0', name: 'workflow_snapshot_get', arguments: {} }], stopReason: 'toolUse' },
          { content: [{ type: 'toolCall', id: 'wait-1', name: 'workflow_snapshot_get', arguments: {} }], stopReason: 'toolUse' },
          { content: [{ type: 'toolCall', id: 'read-5', name: 'search_knowledge', arguments: { query: 'file' } }], stopReason: 'toolUse' },
          'Done.',
        ],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const sessionId = 'pi-progress-after-wait'
  const conversationId = 'pi-progress-after-wait-conv'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId: sessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-progress-after-wait-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId,
    payload: {
      text: 'Read repeatedly, then check, then read again.',
      messages: [{ role: 'user', content: 'Read repeatedly, then check, then read again.' }],
      runBudget: { maxIdenticalToolCalls: 3 },
    },
  })

  const respond = async (callId, payload) => {
    const request = await runtime.waitFor((message) => message.kind === 'request'
      && message.type === 'tool.execute' && message.runId === runId && message.payload?.toolCallId === callId)
    runtime.respond(request, 'tool.execute_completed', payload)
  }

  // Four identical reads: the first is a first sight, the next three reach
  // the streak bound (3) — the run is now armed for a correction.
  for (let index = 1; index <= 4; index += 1) {
    await respond(`read-${index}`, { isError: false, result: { items: [{ documentId: 'doc-1', content: 'old evidence' }] } })
  }
  // A wait-semantic tool whose outcome changes: this is real progress and
  // must clear the read streak and the correction ladder.
  await respond('wait-0', { isError: false, result: { workflow: { id: 'w-1', status: 'running' } } })
  await respond('wait-1', { isError: false, result: { workflow: { id: 'w-1', status: 'completed' } } })
  // The fifth read MUST execute (real progress cleared the ladder) and the
  // harness reports the new state the check produced.
  await respond('read-5', { isError: false, result: { items: [{ documentId: 'doc-1', content: 'new evidence after state change' }] } })

  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  const searchExecutions = runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'tool.execute' && message.runId === runId && message.payload?.tool === 'search_knowledge')
  assert.equal(searchExecutions.length, 5, 'all five reads must execute')
  const session = JSON.parse(await readFile(join(directory, 'session.json'), 'utf8'))
  const finalRead = [...session.messages].reverse().find((message) => message.role === 'toolResult'
    && message.toolName === 'search_knowledge')
  assert.ok(finalRead.content[0].text.includes('new evidence after state change'))
  assert.equal(runtime.messages.filter((message) => message.runId === runId && message.payload?.type === 'run.failed').length, 0)
})

test('a stuck loop after real progress is still bounded and idle polls cannot dodge it', async (context) => {
  // ABC R6 (continued): real progress resets the ladder once, but a new
  // stuck loop re-arms it from zero; idle wait-semantic polls inserted in
  // between must NOT clear the streak (otherwise a model could dodge the
  // bound by interleaving empty polls).
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-stuck-rearm-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: [
          { content: [{ type: 'toolCall', id: 'step-1', name: 'search_knowledge', arguments: { query: 'a' } }], stopReason: 'toolUse' },
          ...Array.from({ length: 4 }, (_, index) => ({
            content: [{ type: 'toolCall', id: `poll-${index + 1}`, name: 'workflow_snapshot_get', arguments: {} }],
            stopReason: 'toolUse',
          })),
          ...Array.from({ length: 8 }, (_, index) => ({
            content: [{ type: 'toolCall', id: `read-a-${index + 1}`, name: 'search_knowledge', arguments: { query: 'a' } }],
            stopReason: 'toolUse',
          })),
          'Done.',
        ],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const conversationId = 'pi-stuck-rearm-conv'
  const sessionId = 'pi-stuck-rearm-session'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId: sessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-stuck-rearm-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId,
    payload: {
      text: 'Do useful work, then get stuck.',
      messages: [{ role: 'user', content: 'Do useful work, then get stuck.' }],
      runBudget: { maxIdenticalToolCalls: 3 },
    },
  })

  const respond = async (callId, payload) => {
    const request = await runtime.waitFor((message) => message.kind === 'request'
      && message.type === 'tool.execute' && message.runId === runId && message.payload?.toolCallId === callId)
    runtime.respond(request, 'tool.execute_completed', payload)
  }

  await respond('step-1', { isError: false, result: { items: [{ documentId: 'd', content: 'first sight' }] } })
  // Four identical idle polls: waiting tools never fire the correction, and
  // an unchanged wait result must not clear the read streak either. They do
  // not arm the ladder by themselves (they are exempt).
  for (let index = 1; index <= 4; index += 1) {
    await respond(`poll-${index}`, { isError: false, result: { workflow: { id: 'w-1', status: 'running' } } })
  }
  // read-a-1 changes the outcome for query 'a' (real progress, ladder reset),
  // read-a-2..4 reproduce it and re-reach the bound, then the next proposals
  // are refused without executing: corrections 1..3, then the run stops.
  for (let index = 1; index <= 8; index += 1) {
    if (index <= 4) {
      await respond(`read-a-${index}`, { isError: false, result: { items: [{ documentId: 'd', content: 'same evidence' }] } })
    }
  }
  const failure = await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.failed')
  assert.equal(failure.payload.code, 'runtime.no_progress')
  const readExecutions = runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'tool.execute' && message.runId === runId && message.payload?.tool === 'search_knowledge')
  assert.equal(readExecutions.length, 5, 'step-1 plus read-a-1..4 execute; the rest are refused')
  const pollExecutions = runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'tool.execute' && message.runId === runId && message.payload?.tool === 'workflow_snapshot_get')
  assert.equal(pollExecutions.length, 4, 'idle polls execute without correction')
})

test('wait-semantic polls with unchanged state are never corrected', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-wait-semantics-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
        fauxResponses: [
          ...Array.from({ length: 5 }, (_, index) => ({
            content: [{ type: 'toolCall', id: `wait-${index + 1}`, name: 'workflow_snapshot_get', arguments: {} }],
            stopReason: 'toolUse',
          })),
          'The workflow is still running; stop polling.',
        ],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const runtimeSessionId = 'pi-wait-session'
  const conversationId = 'pi-wait-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-wait-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'Poll the workflow until told otherwise.',
      messages: [{ role: 'user', content: 'Poll the workflow until told otherwise.' }],
    },
  })

  // Identical input + identical result five times in a row: waiting on
  // external state is exempt from the no-progress guard (Host duration and
  // token budgets still bound the run).
  for (let index = 1; index <= 5; index += 1) {
    const request = await runtime.waitFor((message) => message.kind === 'request'
      && message.type === 'tool.execute' && message.runId === runId
      && message.payload?.toolCallId === `wait-${index}`)
    runtime.respond(request, 'tool.execute_completed', {
      isError: false,
      result: { workflow: { id: 'workflow-1', status: 'running' } },
    })
  }
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  const executions = runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'tool.execute' && message.runId === runId && message.payload?.tool === 'workflow_snapshot_get')
  assert.equal(executions.length, 5)
})

test('records tool and prompt diagnostics while allowing a text-only run with no tools', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-no-tools-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

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
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

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
      projectContext: { projectRoot: directory },
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
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

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
  let runtime
  context.after(() => closeRuntimeAndRemoveDirectory(runtime, directory))
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

  runtime = startRuntime()
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
  let runtime
  context.after(() => closeRuntimeAndRemoveDirectory(runtime, directory))
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

  runtime = startRuntime()
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
  const usage = await runtime.waitFor(message => message.runId === 'run-openai-1' && message.payload?.type === 'usage.request')
  assert.equal(usage.payload.record.usage.input, 4)
  assert.equal(usage.payload.record.usage.output, 6)
  assert.equal(usage.payload.record.cost.knownCost, null)
  assert.equal(usage.payload.record.stage, 'agent')
  assert.doesNotMatch(JSON.stringify(usage.payload.record), /test-key|正式回答|检查项目文件/)
})
