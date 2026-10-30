import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'
import { canonicalPermission } from '../src/control-binding.mjs'
import { estimatePromptTokens } from '../src/prompt-budget.mjs'

const runtimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))
const crashRuntimePath = fileURLToPath(new URL('./fixtures/pi-runtime-crash.mjs', import.meta.url))
const resourceRuntimePath = fileURLToPath(new URL('./fixtures/pi-runtime-resource.mjs', import.meta.url))
const LEGACY_WORKFLOW_MUTATORS = [
  'workflow_start',
  'workflow_stage_start',
  'workflow_stage_complete',
  'workflow_stage_fail',
  'workflow_cancel',
]

function startRuntime({ modelAdmission, modelCompletion, crashPoint, resourceProbe = false } = {}) {
  const child = spawn(process.execPath, [crashPoint ? crashRuntimePath : resourceProbe ? resourceRuntimePath : runtimePath], {
    stdio: ['pipe', 'pipe', 'pipe'], ...(crashPoint ? { env: { ...process.env, FOX_TEST_CRASH_POINT: crashPoint } } : {}),
  })
  const childClosed = new Promise((resolve) => child.once('close', resolve))
  // A spawn error still emits close, but must not become an unhandled event.
  child.on('error', () => {})
  const messages = []
  const waiters = []
  const resourceSamples = []
  const resourceWaiters = []
  const diagnostics = createInterface({ input: child.stderr, crlfDelay: Infinity })
  diagnostics.on('line', line => {
    if (!line.startsWith('FOX_RESOURCE_SAMPLE:')) return
    resourceSamples.push(JSON.parse(line.slice('FOX_RESOURCE_SAMPLE:'.length)))
    for (const waiter of [...resourceWaiters]) waiter()
  })
  const respond = (request, type, payload = {}) => {
    const response = createEnvelope('response', type, {
      requestId: request.id, conversationId: request.conversationId,
      runtimeSessionId: request.runtimeSessionId, runId: request.runId, payload,
    })
    child.stdin.write(`${JSON.stringify(response)}\n`)
    return response
  }
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    const message = JSON.parse(line)
    messages.push(message)
    for (const waiter of [...waiters]) waiter()
    if (message.kind === 'request' && message.type === 'model_request.admit' && modelAdmission !== false) {
      respond(message, 'model_request.admitted', typeof modelAdmission === 'function' ? modelAdmission(message) : {
        allowed: true, timeoutMs: 120000, firstResponseMs: 120000, idleMs: 120000,
        maxOutputTokens: message.payload.maxOutputTokens,
      })
    }
    if (message.kind === 'request' && message.type === 'model_request.complete' && modelCompletion !== false) {
      respond(message, 'model_request.completed', typeof modelCompletion === 'function' ? modelCompletion(message) : { completed: true })
    }
  })
  return {
    child,
    exited: childClosed,
    messages,
    resourceSamples,
    async waitForResource(activeRuns) {
      const existing = resourceSamples.find(sample => sample.activeRuns === activeRuns)
      if (existing) return existing
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('timed out waiting for runtime resource sample')), 4000)
        resourceWaiters.push(() => {
          const sample = resourceSamples.find(sample => sample.activeRuns === activeRuns)
          if (sample) { clearTimeout(timer); resolve(sample) }
        })
      })
    },
    send(type, fields = {}) {
      const request = createEnvelope('request', type, fields)
      child.stdin.write(`${JSON.stringify(request)}\n`)
      return request
    },
    respond,
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
      diagnostics.close()
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

function nativeControlBinding(conversationId, runId) {
  const permission = { mode: 'ask', projectRoot: null, grants: [], approvalEpoch: null }
  return {
    schemaVersion: 1, runId, conversationId, engineId: 'pi', executionProfileId: 'legacy',
    authority: 'authoritative', readOnlyExecutor: 'runtime',
    permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(canonicalPermission(permission))).digest('hex')}`,
    permission,
    budgets: { modelRequestMs: 120000, toolExecutionMs: 120000, runExecutionMs: 600000, approvalWaitMs: 60000 },
    executorRoute: 'pi_native',
  }
}

function nativeProjectBinding(conversationId, runId, projectRoot) {
  const binding = nativeControlBinding(conversationId, runId)
  binding.permission = { ...binding.permission, mode: 'read_only', projectRoot }
  binding.permissionSnapshotId = `sha256:${createHash('sha256').update(JSON.stringify(canonicalPermission(binding.permission))).digest('hex')}`
  return binding
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
    payload: { modelService: { baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 32768, maxOutputTokens: 512 } },
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
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 32768, maxOutputTokens: 512,
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
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 32768, maxOutputTokens: 512,
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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

test('T1.9 rejects an over-window system prompt before any model request', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-context-too-small-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 16_384,
        maxOutputTokens: 2_048,
        fauxResponses: ['This answer must not be requested.'],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const sessionId = 'pi-context-too-small-session'
  const conversationId = 'pi-context-too-small-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId: sessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-context-too-small-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId,
    payload: {
      text: 'This request cannot fit.',
      messages: [{ role: 'user', content: 'This request cannot fit.' }],
      assistantPackage: { packageManifest: { allowedTools: [] } },
      systemPrompt: '中'.repeat(20_000),
    },
  })

  const failed = await runtime.waitFor((message) => message.runId === runId
    && message.payload?.type === 'run.failed')
  assert.equal(failed.payload.code, 'model.context_window_too_small')
  assert.match(failed.payload.message, /approximately \d+ tokens/)
  assert.match(failed.payload.message, /10240 input tokens/)
  assert.equal(runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'model_request.admit').length, 0)
  assert.equal(runtime.messages.filter((message) => message.payload?.type === 'usage.request').length, 0)
})

test('T1.9b the default tool catalog alone cannot fit a 16K/2048 window', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-context-tools-too-small-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-test',
        apiType: 'faux',
        contextWindow: 16_384,
        maxOutputTokens: 2_048,
        fauxResponses: ['This answer must not be requested.'],
      },
    },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const sessionId = 'pi-context-tools-too-small-session'
  const conversationId = 'pi-context-tools-too-small-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId: sessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  const runId = 'pi-context-tools-too-small-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId,
    payload: {
      text: 'U08',
      messages: [{ role: 'user', content: 'U08' }],
    },
  })

  const failed = await runtime.waitFor((message) => message.runId === runId
    && message.payload?.type === 'run.failed')
  assert.equal(failed.payload.code, 'model.context_window_too_small')
  assert.equal(runtime.messages.filter((message) => message.kind === 'request'
    && message.type === 'model_request.admit').length, 0)
})

test('T1.10 CJK-aware prompt estimate covers at least 90% of the measured Chinese sample', () => {
  // dbd61248: 29,823 prompt chars -> 17,886 provider input tokens. The measured
  // composition implies 13,907 CJK code points and 15,916 other characters.
  const sample = '中'.repeat(13_907) + 'a'.repeat(15_916)
  const estimated = estimatePromptTokens(sample)
  assert.equal(sample.length, 29_823)
  assert.ok(estimated >= Math.ceil(17_886 * 0.9), `${estimated}`)
  assert.equal(estimated, 17_886)
  assert.equal(estimatePromptTokens('中'.repeat(1_000)), 1_000)
  assert.equal(estimatePromptTokens('a'.repeat(4_000)), 1_000)
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
        contextWindow: 32768,
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
        contextWindow: 32768,
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
    assert.equal(request.url, '/v1/messages?beta=true')
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
    payload: { modelService: { baseUrl: `http://127.0.0.1:${address.port}`, modelId: 'MiniMax-M3', apiType: 'anthropic-messages', apiKey: 'test-key', contextWindow: 32768, maxOutputTokens: 512 } },
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
    payload: { modelService: { baseUrl: `http://127.0.0.1:${address.port}/v1`, modelId: 'MiniMax-M3', apiType: 'openai-completions', apiKey: 'test-key', contextWindow: 32768, maxOutputTokens: 512 } },
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

// 13.19 I: with no usable external execution backend the Host reports
// run_command unavailable; the Fox session's tool list then leaves it out and
// excludedTools records {name: run_command, reason: host_runtime_unavailable}.
// Available, unknown (legacy payload) or a stale identity keep it.
test('run_command follows the Host runtime availability in the real request snapshot', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-host-runtime-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const cases = [
    ['unavailable', { availability: 'unavailable', reasonCode: 'sandbox_unavailable', reason: 'no verified backend', actions: { start: 'unavailable', status: 'available' } }, false],
    ['available', { availability: 'available', actions: { start: 'available' } }, true],
    ['unknown', { availability: 'unknown' }, true],
    ['absent', null, true],
  ]
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 32768, maxOutputTokens: 512,
    fauxResponses: cases.map(([name]) => `done ${name}`),
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identity = { conversationId: 'host-runtime-c', runtimeSessionId: 'host-runtime-s' }
  const create = runtime.send('create_session', { ...identity, payload: { sessionPath: join(directory, 'session.json') } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  for (const [name, runCommand, expected] of cases) {
    const runId = `host-runtime-${name}`
    const hostRuntimeCapabilities = runCommand && { schemaVersion: 1, capabilityManifestHash: 'sha256:m', executionProfileId: 'legacy', tools: { run_command: runCommand } }
    runtime.send('prompt', { ...identity, runId, payload: {
      text: 'list files', projectContext: { projectRoot: directory },
      ...(hostRuntimeCapabilities ? { hostRuntimeCapabilities } : {}),
    } })
    const snapshot = await runtime.waitFor(message => message.runId === runId && message.payload?.type === 'run.request_snapshot')
    assert.equal(snapshot.payload.effectiveToolNames.includes('run_command'), expected, name)
    const excluded = snapshot.payload.excludedTools.filter(tool => tool.name === 'run_command')
    if (expected) assert.deepEqual(excluded, [], name)
    else assert.deepEqual(excluded, [{ name: 'run_command', reason: 'host_runtime_unavailable', reasonCode: 'sandbox_unavailable' }], name)
    // Only run_command depends on it: read tools stay.
    assert.ok(snapshot.payload.effectiveToolNames.includes('read'), name)
    await runtime.waitFor(message => message.runId === runId && message.payload?.type === 'run.completed')
  }
})

test('a pi_native Run registers its batch and routes work tools through the Host ledger', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-native-batch-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)

  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 32768, maxOutputTokens: 512,
    fauxResponses: [
      { content: [
        { type: 'toolCall', id: 'native-skill', name: 'skill_load', arguments: {} },
        { type: 'toolCall', id: 'native-work', name: 'work_snapshot_get', arguments: {} },
      ], stopReason: 'toolUse' },
      'Both native tool results were received.',
    ],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')

  const conversationId = 'native-batch-conversation'
  const runtimeSessionId = 'native-batch-session'
  const create = runtime.send('create_session', {
    conversationId, runtimeSessionId, payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')

  const runId = 'native-batch-run'
  const controlBinding = nativeControlBinding(conversationId, runId)
  runtime.send('prompt', {
    conversationId, runtimeSessionId, runId,
    payload: { text: 'Load a skill and read the work snapshot.', controlBinding },
  })

  // The reported failure mode was zero registrations: the SDK installs its own
  // tool hook, so nothing reached the Host until the adapter composed both.
  const registration = await runtime.waitFor(message => (
    message.kind === 'request' && message.type === 'tool_batch.register' && message.runId === runId
  ))
  assert.equal(registration.payload.contextGeneration, 1)
  assert.deepEqual(
    registration.payload.calls.map(({ callId, tool, sourceOrder }) => ({ callId, tool, sourceOrder })),
    [
      { callId: 'native-skill', tool: 'skill_load', sourceOrder: 0 },
      // Work tools reach the session through the planning extension, so this
      // position proves they were wrapped on that path too.
      { callId: 'native-work', tool: 'work_snapshot_get', sourceOrder: 1 },
    ],
  )
  for (const call of registration.payload.calls) assert.match(call.inputDigest, /^sha256:[0-9a-f]{64}$/)
  await new Promise(resolve => setTimeout(resolve, 50))
  assert.ok(!runtime.messages.some(message => message.runId === runId
    && ['tool.started', 'tool.updated', 'tool.completed'].includes(message.payload?.type)),
  'the strict Host projection must not see lifecycle events before it acknowledges registration')
  assert.ok(!runtime.messages.some(message => message.runId === runId && message.type === 'tool_call.execute'))
  runtime.respond(registration, 'tool_batch.registered', { batchId: 'native-batch-1', registeredCalls: 2 })

  for (const tool of ['skill_load', 'work_snapshot_get']) {
    const execute = await runtime.waitFor(message => (
      message.kind === 'request' && message.type === 'tool_call.execute'
      && message.runId === runId && message.payload?.tool === tool
    ))
    assert.equal(execute.payload.batchId, 'native-batch-1')
    runtime.respond(execute, 'tool.execute_completed', { isError: false, result: { servedBy: tool } })
  }

  await runtime.waitFor(message => message.runId === runId && message.payload?.type === 'run.completed')
  await runtime.waitFor(message => message.runId === runId && message.type === 'run_released')
  const lifecycle = runtime.messages.filter(message => message.runId === runId && message.payload?.type?.startsWith('tool.'))
  assert.ok(lifecycle.some(message => message.payload.type === 'tool.started' && message.payload.toolCallId === 'native-skill'))
  assert.ok(lifecycle.some(message => message.payload.type === 'tool.started' && message.payload.toolCallId === 'native-work'))
  const events = runtime.messages.filter(message => message.runId === runId && message.type === 'runtime_event')
  assert.deepEqual(events.map(message => message.seq), events.map((_, index) => index + 1))
  // A routed Run must never fall back to the Legacy admission path.
  assert.ok(!runtime.messages.some(message => (
    message.kind === 'request' && message.type === 'tool.execute' && message.runId === runId
  )))
  const session = JSON.parse(await readFile(join(directory, 'session.json'), 'utf8'))
  const results = session.messages
    .filter(message => message.role === 'toolResult')
    .map(message => ({
      toolName: message.toolName,
      isError: message.isError,
      result: message.content?.[0]?.text ? JSON.parse(message.content[0].text) : null,
    }))
  assert.deepEqual(results, [
    { toolName: 'skill_load', isError: false, result: { servedBy: 'skill_load' } },
    { toolName: 'work_snapshot_get', isError: false, result: { servedBy: 'work_snapshot_get' } },
  ])
})

test('a pi_native Run preserves bounded structured results and Host failure diagnostics', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-native-results-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 32768, maxOutputTokens: 512,
    fauxResponses: [
      { content: [
        { type: 'toolCall', id: 'native-large', name: 'read_attachment', arguments: { attachmentId: 'native-source' } },
        { type: 'toolCall', id: 'native-failed', name: 'work_snapshot_get', arguments: {} },
      ], stopReason: 'toolUse' },
      'The work snapshot failed; the skill result can be read by reference.',
    ],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const conversationId = 'native-results-conversation'
  const runtimeSessionId = 'native-results-session'
  const runId = 'native-results-run'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { conversationId, runtimeSessionId, payload: { sessionPath } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  runtime.send('prompt', {
    conversationId, runtimeSessionId, runId,
    payload: { text: 'Load a skill and check the work snapshot.', controlBinding: nativeControlBinding(conversationId, runId) },
  })
  const registration = await runtime.waitFor(message => message.type === 'tool_batch.register' && message.runId === runId)
  runtime.respond(registration, 'tool_batch.registered', { batchId: 'native-results-batch', registeredCalls: 2 })
  const large = await runtime.waitFor(message => message.type === 'tool_call.execute' && message.payload?.callId === 'native-large')
  const result = {
    documentation: 'x'.repeat(121_000),
    next: { offset: 550 },
  }
  const storedBytes = Buffer.byteLength(JSON.stringify(result), 'utf8')
  assert.ok(storedBytes > 120_000 && storedBytes <= 131_072)
  runtime.respond(large, 'tool.execute_completed', {
    result,
    toolResultStorage: { stored: true, storedBytes, retrievableBytes: storedBytes },
  })
  const failed = await runtime.waitFor(message => message.type === 'tool_call.execute' && message.payload?.callId === 'native-failed')
  const resultRef = `fox-result://${runId}/native-failed`
  runtime.respond(failed, 'tool.execute_failed', {
    isError: true,
    error: 'Work snapshot is unavailable.',
    errorDetails: { code: 'work.snapshot_unavailable', retryable: false },
    result: { content: [{ type: 'text', text: 'The referenced work record no longer exists.' }] },
    resultRef,
  })
  await runtime.waitFor(message => message.runId === runId && message.payload?.type === 'run.completed')
  const session = JSON.parse(await readFile(sessionPath, 'utf8'))
  const results = session.messages.filter(message => message.role === 'toolResult')
  assert.equal(results.length, 2)
  const largeResult = results.find(message => message.toolCallId === 'native-large')
  assert.equal(largeResult.isError, false)
  assert.ok(Buffer.byteLength(largeResult.content[0].text, 'utf8') <= 9_000)
  const view = JSON.parse(largeResult.content[0].text)
  assert.equal(view.foxModelView.storageVerified, true)
  assert.equal(view.foxModelView.retrievable, true)
  assert.equal(view.foxModelView.resultRef, `fox-result://${runId}/native-large`)
  assert.deepEqual(view.next, { offset: 550 })
  assert.ok(view.documentation.length < result.documentation.length)
  const failure = results.find(message => message.toolCallId === 'native-failed')
  assert.equal(failure.isError, true)
  const diagnostic = failure.content.map(block => block.text ?? '').join('\n')
  assert.match(diagnostic, /Work snapshot is unavailable/)
  assert.match(diagnostic, /referenced work record no longer exists/)
  assert.match(diagnostic, /work\.snapshot_unavailable/)
  assert.ok(diagnostic.includes(resultRef))
  assert.ok(!runtime.messages.some(message => message.kind === 'request' && message.type === 'tool.execute' && message.runId === runId))
})

test('a native model continuation waits until an unexecuted tool position is closed by the Host', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-close-barrier-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://close-barrier', modelId: 'close-model', apiType: 'faux',
    fauxResponses: [{ content: [{ type: 'toolCall', id: 'missing-tool-id', name: 'missing_native_tool', arguments: {} }], stopReason: 'toolUse' }, 'The missing tool was not executed.'],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identity = { conversationId: 'close-barrier-c', runtimeSessionId: 'close-barrier-s', runId: 'close-barrier-r' }
  const create = runtime.send('create_session', { ...identity, payload: { sessionFormat: 'pi-native', cwd: directory, sessionDir: join(directory, 'sessions') } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  runtime.send('prompt', { ...identity, payload: { text: 'Check this missing tool.', controlBinding: nativeControlBinding(identity.conversationId, identity.runId) } })
  const registration = await runtime.waitFor(message => message.type === 'tool_batch.register')
  runtime.respond(registration, 'tool_batch.registered', { batchId: 'missing-batch', registeredCalls: 1 })
  const close = await runtime.waitFor(message => message.type === 'tool_call.close')
  assert.equal(close.payload.reason, 'unknown_tool')
  await new Promise(resolve => setTimeout(resolve, 50))
  assert.equal(runtime.messages.filter(message => message.type === 'model_request.admit').length, 1, 'the next model request cannot pass outstanding Host tool cleanup')
  runtime.respond(close, 'tool_call.closed', { closed: true, state: 'not_started' })
  await runtime.waitFor(message => message.type === 'run_released' && message.runId === identity.runId)
  assert.equal(runtime.messages.filter(message => message.type === 'model_request.admit').length, 2)
  assert.ok(runtime.messages.some(message => message.payload?.type === 'run.completed' && message.runId === identity.runId))
  const nextRun = 'close-barrier-follow-up'
  runtime.send('prompt', { ...identity, runId: nextRun, payload: {
    text: 'Continue after the unavailable tool.', controlBinding: nativeControlBinding(identity.conversationId, nextRun),
    modelService: { baseUrl: 'faux://follow-up', modelId: 'follow-up-model', apiType: 'faux', fauxResponses: ['Verified nonexecution does not block this follow-up.'] },
  } })
  const facts = await runtime.waitFor(message => message.type === 'tool_call.facts' && message.runId === nextRun)
  assert.equal(facts.payload.calls[0].hasResult, true)
  runtime.respond(facts, 'tool_call.facts_completed', { contextGeneration: 1, blocked: false,
    facts: [{ callId: 'missing-tool-id', state: 'not_executed', response: { isError: true, error: 'tool was not executed: unknown_tool' } }] })
  await runtime.waitFor(message => message.type === 'run_released' && message.runId === nextRun)
  assert.ok(runtime.messages.some(message => message.runId === nextRun && message.payload?.type === 'run.completed'))
})

test('an entirely unknown and invalid native batch registers before lifecycle projection and closes every position', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-invalid-order-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://invalid-order', modelId: 'invalid-order', apiType: 'faux',
    fauxResponses: [{ content: [
      { type: 'toolCall', id: 'invalid-unknown', name: 'missing_native_tool', arguments: {} },
      { type: 'toolCall', id: 'invalid-known', name: 'read_attachment', arguments: { attachmentId: {} } },
    ], stopReason: 'toolUse' }, 'Neither unavailable call executed.'],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identity = { conversationId: 'invalid-order-c', runtimeSessionId: 'invalid-order-s', runId: 'invalid-order-r' }
  const create = runtime.send('create_session', { ...identity, payload: { sessionPath: join(directory, 'session.json') } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  runtime.send('prompt', { ...identity, payload: { text: 'Check unavailable tools.', controlBinding: nativeControlBinding(identity.conversationId, identity.runId) } })
  const registration = await runtime.waitFor(message => message.type === 'tool_batch.register')
  assert.deepEqual(registration.payload.calls.map(call => call.callId), ['invalid-unknown', 'invalid-known'])
  await new Promise(resolve => setTimeout(resolve, 50))
  assert.ok(!runtime.messages.some(message => ['tool.started', 'tool.updated', 'tool.completed'].includes(message.payload?.type)))
  assert.ok(!runtime.messages.some(message => message.type === 'tool_call.close' || message.type === 'tool_call.execute'))
  runtime.respond(registration, 'tool_batch.registered', { batchId: 'invalid-order-batch', registeredCalls: 2 })
  for (const [callId, reason] of [['invalid-unknown', 'unknown_tool'], ['invalid-known', 'validation_failed']]) {
    const close = await runtime.waitFor(message => message.type === 'tool_call.close' && message.payload?.callId === callId)
    assert.equal(close.payload.reason, reason)
    runtime.respond(close, 'tool_call.closed', { closed: true, state: 'not_started' })
  }
  await runtime.waitFor(message => message.type === 'run_released' && message.runId === identity.runId)
  assert.ok(runtime.messages.some(message => message.runId === identity.runId && message.payload?.type === 'run.completed'))
  assert.ok(!runtime.messages.some(message => message.type === 'tool_call.execute'))
  const lifecycle = runtime.messages.filter(message => message.payload?.toolCallId === 'invalid-known')
  const start = lifecycle.find(message => message.payload.type === 'tool.started')
  const end = lifecycle.find(message => message.payload.type === 'tool.completed')
  assert.ok(start && end && start.seq < end.seq)
  const events = runtime.messages.filter(message => message.runId === identity.runId && message.type === 'runtime_event')
  assert.deepEqual(events.map(message => message.seq), events.map((_, index) => index + 1))
})

test('concurrent sessions keep Run model bindings and closing one session preserves the other', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-multi-session-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://default', modelId: 'default-model', apiType: 'faux',
    fauxResponses: ['The process default must not replace a Run model.'],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identities = ['a', 'b'].map(name => ({ conversationId: `multi-c-${name}`, runtimeSessionId: `multi-s-${name}`, runId: `multi-r-${name}` }))
  const tools = ['skill_load', 'work_snapshot_get']
  for (let index = 0; index < identities.length; index += 1) {
    const identity = identities[index]
    const create = runtime.send('create_session', { ...identity, payload: { sessionPath: join(directory, `${index}.json`) } })
    await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
    runtime.send('prompt', { ...identity, payload: {
      text: `Run for ${index}.`, controlBinding: nativeControlBinding(identity.conversationId, identity.runId),
      modelService: { baseUrl: `faux://run-${index}`, modelId: `run-model-${index}`, apiType: 'faux',
        fauxResponses: [{ content: [{ type: 'toolCall', id: `multi-call-${index}`, name: tools[index], arguments: {} }], stopReason: 'toolUse' }, `Answer from model ${index}.`] },
    } })
  }
  for (let index = 0; index < identities.length; index += 1) {
    const registration = await runtime.waitFor(message => message.type === 'tool_batch.register' && message.runId === identities[index].runId)
    runtime.respond(registration, 'tool_batch.registered', { batchId: `multi-batch-${index}`, registeredCalls: 1 })
  }
  const executing = await Promise.all(identities.map(identity => runtime.waitFor(message => message.type === 'tool_call.execute' && message.runId === identity.runId)))
  const snapshots = await Promise.all(identities.map(identity => runtime.waitFor(message => message.runId === identity.runId && message.payload?.type === 'run.request_snapshot')))
  assert.deepEqual(snapshots.map(message => message.payload.model), ['run-model-0', 'run-model-1'])
  assert.deepEqual(snapshots.map(message => message.payload.baseUrl), ['faux://run-0', 'faux://run-1'])
  assert.notEqual(snapshots[0].payload.provider, snapshots[1].payload.provider, 'faux streams must have independent providers')
  const duplicate = runtime.send('prompt', { ...identities[0], runId: 'multi-duplicate', payload: { text: 'overlap' } })
  const duplicateResponse = await runtime.waitFor(message => message.requestId === duplicate.id)
  assert.equal(duplicateResponse.type, 'request_failed')
  assert.equal(duplicateResponse.payload.code, 'session.busy')
  const reinitialize = runtime.send('initialize', { payload: { modelService: { baseUrl: 'faux://replacement', modelId: 'replacement', apiType: 'faux' } } })
  const busy = await runtime.waitFor(message => message.requestId === reinitialize.id)
  assert.equal(busy.type, 'request_failed')
  assert.equal(busy.payload.code, 'session.runtime_busy')
  const cancelRun = runtime.send('cancel', identities[0])
  await runtime.waitFor(message => message.requestId === cancelRun.id && message.type === 'request_succeeded')
  const cancel = await runtime.waitFor(message => message.type === 'tool_call.cancel' && message.runId === identities[0].runId)
  assert.ok(runtime.messages.some(message => message.runId === identities[0].runId && message.payload?.type === 'run.cancelled'))
  assert.ok(!runtime.messages.some(message => message.type === 'run_released' && message.runId === identities[0].runId))
  const close = runtime.send('session.close', { conversationId: identities[0].conversationId, runtimeSessionId: identities[0].runtimeSessionId })
  await new Promise(resolve => setTimeout(resolve, 50))
  assert.ok(!runtime.messages.some(message => message.requestId === close.id), 'closing a cancelled Run still waits for its outstanding Host cleanup')
  assert.ok(!runtime.messages.some(message => message.type === 'run_released' && message.runId === identities[0].runId), 'terminal notification does not release the session before Host cleanup acknowledgement')
  runtime.respond(cancel, 'tool_call.cancelled', { cancelled: true, resultRecorded: false, outcomeUnknown: true })
  const closed = await runtime.waitFor(message => message.requestId === close.id)
  assert.equal(closed.type, 'session_closed')
  assert.equal(closed.payload.closed, true)
  const released = runtime.messages.filter(message => message.type === 'run_released' && message.runId === identities[0].runId)
  assert.equal(released.length, 1)
  assert.equal(released[0].kind, 'event')
  assert.equal(released[0].conversationId, identities[0].conversationId)
  assert.equal(released[0].runtimeSessionId, identities[0].runtimeSessionId)
  assert.equal(released[0].payload.contextGeneration, 1)
  assert.ok(!runtime.messages.some(message => message.runId === identities[1].runId && ['run.failed', 'run.cancelled'].includes(message.payload?.type)))
  runtime.respond(executing[1], 'tool.execute_completed', { result: { servedBy: 'run-b' } })
  await runtime.waitFor(message => message.runId === identities[1].runId && message.payload?.type === 'run.completed')
  await runtime.waitFor(message => message.type === 'run_released' && message.runId === identities[1].runId)
  assert.equal(runtime.messages.filter(message => message.type === 'run_released' && message.runId === identities[1].runId).length, 1)
  const b = JSON.parse(await readFile(join(directory, '1.json'), 'utf8'))
  assert.ok(JSON.stringify(b.messages).includes('Answer from model 1.'))
  assert.ok(!JSON.stringify(b.messages).includes('process default'))
  const afterClose = runtime.send('prompt', { ...identities[0], runId: 'multi-after-close', payload: { text: 'must fail' } })
  const rejected = await runtime.waitFor(message => message.requestId === afterClose.id)
  assert.equal(rejected.type, 'request_failed')
  assert.match(rejected.payload.message, /not open/)
})

test('concurrent native Runs send each actual HTTP request to its bound endpoint with its own model and credential', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-model-isolation-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const received = []
  let release
  const bothArrived = new Promise(resolve => { release = resolve })
  const endpoints = []
  for (let index = 0; index < 2; index += 1) {
    const requests = []
    received.push(requests)
    const server = createServer(async (request, response) => {
      let raw = ''
      for await (const chunk of request) raw += chunk
      requests.push({ body: JSON.parse(raw), authorization: request.headers.authorization })
      if (received.every(items => items.length > 0)) release()
      await bothArrived
      response.writeHead(200, { 'content-type': 'text/event-stream' })
      for (const choice of [
        { index: 0, delta: { role: 'assistant', content: `Endpoint ${index} replied.` }, finish_reason: null },
        { index: 0, delta: {}, finish_reason: 'stop' },
      ]) response.write(`data: ${JSON.stringify({ id: `model-${index}`, object: 'chat.completion.chunk', created: 1,
        model: `bound-model-${index}`, choices: [choice], usage: { prompt_tokens: 11, completion_tokens: 2 } })}\n\n`)
      response.end('data: [DONE]\n\n')
    })
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
    context.after(async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)) })
    endpoints.push(`http://127.0.0.1:${server.address().port}/v1`)
  }
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://default', modelId: 'default-model', apiType: 'faux', fauxResponses: ['Default model must not be used.'],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identities = [0, 1].map(index => ({ conversationId: `http-c-${index}`, runtimeSessionId: `http-s-${index}`, runId: `http-r-${index}` }))
  for (let index = 0; index < 2; index += 1) {
    const identity = identities[index]
    const create = runtime.send('create_session', { ...identity, payload: { sessionPath: join(directory, `${index}.json`) } })
    await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
    runtime.send('prompt', { ...identity, payload: {
      text: `Answer from endpoint ${index}.`, controlBinding: nativeControlBinding(identity.conversationId, identity.runId),
      modelService: { baseUrl: endpoints[index], modelId: `bound-model-${index}`, apiType: 'openai-completions',
        apiKey: `fixture-key-${index}`, contextWindow: 32768, maxOutputTokens: 128 },
    } })
  }
  await Promise.all(identities.map(identity => runtime.waitFor(message => message.runId === identity.runId && message.payload?.type === 'run.completed')))
  for (let index = 0; index < 2; index += 1) {
    assert.equal(received[index].length, 1)
    assert.equal(received[index][0].body.model, `bound-model-${index}`)
    assert.equal(received[index][0].authorization, `Bearer fixture-key-${index}`)
    const snapshot = JSON.parse(await readFile(join(directory, `${index}.json`), 'utf8'))
    assert.ok(JSON.stringify(snapshot.messages).includes(`Endpoint ${index} replied.`))
    assert.ok(!JSON.stringify(snapshot.messages).includes(`Endpoint ${1 - index} replied.`))
  }
})

test('a Pi-native planner tool and main work tool with the same model call ID use separate Host positions', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-planner-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://planner', modelId: 'native-planner', apiType: 'faux', plannerMode: 'always',
    fauxResponses: [
      { content: [{ type: 'toolCall', id: 'shared-model-id', name: 'read', arguments: { path: 'never-executed-locally.txt' } }], stopReason: 'toolUse' },
      '{"summary":"Inspect the source and work state","steps":["Read supplied evidence","Check work state"],"risks":[],"needsGoal":false}',
      { content: [{ type: 'toolCall', id: 'shared-model-id', name: 'work_snapshot_get', arguments: {} }], stopReason: 'toolUse' },
      'The executor used the Host work result.',
    ],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identity = { conversationId: 'native-planner-c', runtimeSessionId: 'native-planner-s', runId: 'native-planner-r' }
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { ...identity, payload: { sessionPath } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  runtime.send('prompt', { ...identity, payload: {
    text: 'Inspect the project, then check the work state.',
    projectContext: { projectRoot: directory, permissionMode: 'read_only' },
    controlBinding: nativeProjectBinding(identity.conversationId, identity.runId, directory),
  } })
  const plannerRegistration = await runtime.waitFor(message => message.type === 'tool_batch.register' && message.payload?.calls?.[0]?.tool === 'read')
  assert.match(plannerRegistration.payload.calls[0].callId, /^planner:[0-9a-f-]+:shared-model-id$/)
  runtime.respond(plannerRegistration, 'tool_batch.registered', { batchId: 'planner-batch', registeredCalls: 1 })
  const read = await runtime.waitFor(message => message.type === 'tool_call.execute' && message.payload?.tool === 'read')
  assert.equal(read.payload.callId, plannerRegistration.payload.calls[0].callId)
  runtime.respond(read, 'tool.execute_completed', { result: { content: [{ type: 'text', text: 'Only the Host supplied this source.' }] } })
  await runtime.waitFor(message => message.payload?.type === 'planner.completed' && message.runId === identity.runId)
  const mainRegistration = await runtime.waitFor(message => message.type === 'tool_batch.register' && message.payload?.calls?.[0]?.tool === 'work_snapshot_get')
  assert.equal(mainRegistration.payload.calls[0].callId, 'shared-model-id')
  runtime.respond(mainRegistration, 'tool_batch.registered', { batchId: 'main-batch', registeredCalls: 1 })
  const work = await runtime.waitFor(message => message.type === 'tool_call.execute' && message.payload?.tool === 'work_snapshot_get')
  runtime.respond(work, 'tool.execute_completed', { result: { work: 'Host work state' } })
  await runtime.waitFor(message => message.payload?.type === 'run.completed' && message.runId === identity.runId)
  assert.ok(!runtime.messages.some(message => message.kind === 'request' && ['tool.preflight', 'tool.execute'].includes(message.type)))
  const stages = runtime.messages.filter(message => message.type === 'model_request.admit').map(message => message.payload.stage)
  assert.deepEqual(stages, ['planner', 'planner', 'agent', 'agent'])
  const records = runtime.messages.filter(message => message.payload?.type === 'usage.request').map(message => message.payload.record)
  const admissions = runtime.messages.filter(message => message.type === 'model_request.admit')
  assert.deepEqual(records.map(record => record.requestId), admissions.map(message => message.payload.requestId))
  const session = JSON.parse(await readFile(sessionPath, 'utf8'))
  assert.equal(session.messages.filter(message => message.role === 'toolResult').length, 1)
  assert.equal(session.messages.find(message => message.role === 'toolResult').toolName, 'work_snapshot_get')
  assert.ok(!JSON.stringify(session.messages).includes('Only the Host supplied this source.'))
})

for (const crash of [
  { point: 'register', state: 'not_executed', effects: 0 },
  { point: 'execute', state: 'not_executed', effects: 0 },
  { point: 'execute', state: 'outcome_unknown', effects: 1 },
  { point: 'execute', state: 'waiting_approval', effects: 0 },
  { point: 'execute', state: 'committed', effects: 1 },
  { point: 'after_result', state: 'committed', effects: 1 },
  { point: 'after_cancel', state: 'committed', effects: 1 },
]) {
  test(`Pi-native process exit at ${crash.point} recovers ${crash.state} without replaying an effect`, async context => {
    const directory = await mkdtemp(join(tmpdir(), 'fox-pi-native-crash-'))
    const runtime = startRuntime({ crashPoint: crash.point, modelAdmission: false })
    registerRuntimeCleanup(context, runtime, directory)
    const initial = runtime.send('initialize', { payload: { modelService: {
      baseUrl: 'faux://crash', modelId: 'crash-model', apiType: 'faux',
      fauxResponses: [{ content: [{ type: 'toolCall', id: 'crash-work-call', name: 'work_snapshot_get', arguments: {} }], stopReason: 'toolUse' }, 'after result'],
    } } })
    await runtime.waitFor(message => message.requestId === initial.id && message.type === 'ready')
    const identity = { conversationId: 'crash-conversation', runtimeSessionId: 'crash-session' }
    const create = runtime.send('create_session', { ...identity,
      payload: { sessionFormat: 'pi-native', cwd: directory, sessionDir: join(directory, 'sessions') } })
    const created = await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
    const runId = 'crash-run'
    runtime.send('prompt', { ...identity, runId,
      payload: { text: 'Read work state.', controlBinding: nativeControlBinding(identity.conversationId, runId) } })
    const admit = await runtime.waitFor(message => message.type === 'model_request.admit')
    runtime.respond(admit, 'model_request.admitted', {
      allowed: true, timeoutMs: 120000, firstResponseMs: 120000, idleMs: 120000, maxOutputTokens: admit.payload.maxOutputTokens,
    })
    const registration = await runtime.waitFor(message => message.type === 'tool_batch.register')
    let effects = 0
    const hostResult = crash.point === 'after_cancel'
      ? { isError: true, error: 'Tool cancelled; retained output is effect evidence.',
          result: { content: [{ type: 'text', text: 'HOST_EFFECT_RECEIPT: work effect completed before cancellation.' }] } }
      : { result: { content: [{ type: 'text', text: 'Host committed work state.' }] } }
    if (crash.point !== 'register') {
      runtime.respond(registration, 'tool_batch.registered', { batchId: 'crash-batch', registeredCalls: 1 })
      const execute = await runtime.waitFor(message => message.type === 'tool_call.execute')
      effects += crash.effects
      if (crash.point === 'after_result') {
        runtime.respond(execute, 'tool.execute_completed', hostResult)
        await runtime.waitFor(message => message.type === 'model_request.admit' && message.id !== admit.id)
      } else if (crash.point === 'after_cancel') {
        runtime.send('cancel', { ...identity, runId })
        const cancel = await runtime.waitFor(message => message.type === 'tool_call.cancel')
        runtime.respond(cancel, 'tool_call.cancelled', { cancelled: true, resultRecorded: false, outcomeUnknown: true })
      }
    }
    assert.equal(await runtime.exited, 73, 'the injected failure is a real process.exit')
    const before = await readFile(created.payload.nativeSessionPath, 'utf8')
    const restored = startRuntime()
    context.after(() => restored.close())
    const initialize = restored.send('initialize', { payload: { modelService: {
      baseUrl: 'faux://recovered', modelId: 'recovered-model', apiType: 'faux', fauxResponses: ['Recovered continuation.'],
    } } })
    await restored.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
    let resume = restored.send('resume_session', { ...identity, runId: 'recovery-query-run', payload: {
      sessionFormat: 'pi-native', nativeSessionPath: created.payload.nativeSessionPath, nativeSessionId: created.payload.nativeSessionId,
      contextGeneration: 1,
    } })
    let facts = await restored.waitFor(message => message.type === 'tool_call.facts')
    if (crash.point === 'register') {
      const competing = restored.send('resume_session', { ...identity, runId: 'recovery-query-run', payload: resume.payload })
      const busy = await restored.waitFor(message => message.requestId === competing.id)
      assert.equal(busy.type, 'request_failed')
      assert.equal(busy.payload.code, 'session.busy')
      const close = restored.send('session.close', identity)
      const failedOpen = await restored.waitFor(message => message.requestId === resume.id)
      assert.equal(failedOpen.type, 'request_failed')
      assert.equal((await restored.waitFor(message => message.requestId === close.id)).type, 'session_closed')
      assert.equal(await readFile(created.payload.nativeSessionPath, 'utf8'), before)
      const cancelledFactsId = facts.id
      resume = restored.send('resume_session', { ...identity, runId: 'recovery-query-run', payload: resume.payload })
      facts = await restored.waitFor(message => message.type === 'tool_call.facts' && message.id !== cancelledFactsId)
    }
    assert.deepEqual(facts.payload.calls.map(call => ({ callId: call.callId, hasResult: call.hasResult })),
      [{ callId: 'crash-work-call', hasResult: ['after_result', 'after_cancel'].includes(crash.point) }])
    const fact = { callId: 'crash-work-call', state: crash.state,
      ...(crash.state === 'committed' ? { response: hostResult } : crash.state === 'not_executed' ? {
        response: { isError: true, error: 'Host confirmed this call did not start.' },
      } : {}),
    }
    const blocked = ['outcome_unknown', 'waiting_approval'].includes(crash.state)
    const factBody = { contextGeneration: 1, blocked, facts: [fact] }
    restored.respond(facts, 'tool_call.facts_result', factBody)
    const resumed = await restored.waitFor(message => message.requestId === resume.id)
    if (blocked) {
      assert.equal(resumed.type, 'request_failed')
      assert.equal(resumed.payload.code, crash.state === 'waiting_approval' ? 'session.native_waiting_approval' : 'session.native_recovery_pending')
      assert.equal(await readFile(created.payload.nativeSessionPath, 'utf8'), before)
    } else {
      assert.equal(resumed.type, 'session_created')
      const continuationId = 'recovered-run'
      restored.send('prompt', { ...identity, runId: continuationId, payload: {
        text: 'Continue from settled work.', controlBinding: nativeControlBinding(identity.conversationId, continuationId),
      } })
      const recheck = await restored.waitFor(message => message.type === 'tool_call.facts' && message.runId === continuationId)
      assert.equal(recheck.payload.calls[0].hasResult, true)
      restored.respond(recheck, 'tool_call.facts_result', factBody)
      await restored.waitFor(message => message.runId === continuationId && message.payload?.type === 'run.completed')
      const entries = (await readFile(created.payload.nativeSessionPath, 'utf8')).trim().split('\n').map(JSON.parse)
      const results = entries.filter(entry => entry.message?.role === 'toolResult' && entry.message.toolCallId === 'crash-work-call')
      if (crash.point === 'after_cancel') {
        assert.equal(results.length, 2, 'the local cancellation remains in abandoned history and the Host receipt is appended on the new branch')
        assert.ok(JSON.stringify(results.at(-1)).includes('HOST_EFFECT_RECEIPT'))
      } else assert.equal(results.length, 1)
    }
    assert.equal(effects, crash.effects)
    assert.ok(!restored.messages.some(message => message.type === 'tool_call.execute'), 'recovery does not execute the original or a replacement call')
  })
}

for (const activeCount of [2, 3, 4]) {
  test(`${activeCount} native AgentSessions remain active in one process and expose an isolated resource sample`, async context => {
    const directory = await mkdtemp(join(tmpdir(), 'fox-native-resource-'))
    const runtime = startRuntime({ resourceProbe: true })
    registerRuntimeCleanup(context, runtime, directory)
    const initialize = runtime.send('initialize', { payload: { modelService: {
      baseUrl: 'faux://resource', modelId: 'resource-model', apiType: 'faux',
      fauxResponses: [{ content: [{ type: 'toolCall', id: 'resource-call', name: 'skill_load', arguments: {} }], stopReason: 'toolUse' }, 'complete'],
    } } })
    await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
    const runs = []
    const started = new Map()
    const latencies = []
    for (let index = 0; index < activeCount; index += 1) {
      const identity = { conversationId: `resource-c-${index}`, runtimeSessionId: `resource-s-${index}`, runId: `resource-r-${index}` }
      runs.push(identity)
      const create = runtime.send('create_session', { ...identity, payload: {
        sessionFormat: 'pi-native', cwd: directory, sessionDir: join(directory, `sessions-${index}`),
      } })
      await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
      started.set(identity.runId, performance.now())
      runtime.send('prompt', { ...identity, payload: { text: `Resource sample ${index}.`, controlBinding: nativeControlBinding(identity.conversationId, identity.runId) } })
    }
    for (const identity of runs) {
      const registration = await runtime.waitFor(message => message.type === 'tool_batch.register' && message.runId === identity.runId)
      runtime.respond(registration, 'tool_batch.registered', { batchId: `resource-batch-${identity.runId}`, registeredCalls: 1 })
    }
    const executing = await Promise.all(runs.map(async identity => {
      const request = await runtime.waitFor(message => message.type === 'tool_call.execute' && message.runId === identity.runId)
      latencies.push(Math.round(performance.now() - started.get(identity.runId)))
      return request
    }))
    assert.equal(executing.length, activeCount)
    const sample = await runtime.waitForResource(activeCount)
    assert.ok(sample.rss > 0 && sample.heapUsed > 0)
    console.log(JSON.stringify({ scenario: 'PiNative sidecar with faux model and stub Host', activeRuns: activeCount,
      rssMiB: +(sample.rss / 1048576).toFixed(2), heapUsedMiB: +(sample.heapUsed / 1048576).toFixed(2),
      promptToToolRequestMs: latencies }))
    for (const request of executing) runtime.respond(request, 'tool.execute_completed', { result: { servedBy: request.runId } })
    await Promise.all(runs.map(identity => runtime.waitFor(message => message.payload?.type === 'run.completed' && message.runId === identity.runId)))
  })
}

test('CX-3 a native prompt refused before acknowledgement is not released as a Run', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-preack-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://preack', modelId: 'preack-model', apiType: 'faux',
    fauxResponses: [{ content: [{ type: 'toolCall', id: 'preack-tool', name: 'missing_native_tool', arguments: {} }], stopReason: 'toolUse' }, 'Done.'],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identity = { conversationId: 'preack-c', runtimeSessionId: 'preack-s', runId: 'preack-r' }
  const create = runtime.send('create_session', { ...identity, payload: { sessionFormat: 'pi-native', cwd: directory, sessionDir: join(directory, 'sessions') } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  runtime.send('prompt', { ...identity, payload: { text: 'Use the missing tool.', controlBinding: nativeControlBinding(identity.conversationId, identity.runId) } })
  const registration = await runtime.waitFor(message => message.type === 'tool_batch.register')
  runtime.respond(registration, 'tool_batch.registered', { batchId: 'preack-batch', registeredCalls: 1 })
  const close = await runtime.waitFor(message => message.type === 'tool_call.close')
  runtime.respond(close, 'tool_call.closed', { closed: true, state: 'not_started' })
  await runtime.waitFor(message => message.type === 'run_released' && message.runId === identity.runId)

  const nextRun = 'preack-follow-up'
  const prompt = runtime.send('prompt', { ...identity, runId: nextRun, payload: {
    text: 'Continue.', controlBinding: nativeControlBinding(identity.conversationId, nextRun) } })
  const facts = await runtime.waitFor(message => message.type === 'tool_call.facts' && message.runId === nextRun)
  runtime.respond(facts, 'tool_call.facts_result', { contextGeneration: 1, blocked: true,
    facts: [{ callId: facts.payload.calls[0].callId, state: 'outcome_unknown' }] })
  const refused = await runtime.waitFor(message => message.requestId === prompt.id)
  assert.equal(refused.type, 'request_failed')
  assert.equal(refused.payload.code, 'session.native_recovery_pending')
  await new Promise(resolve => setTimeout(resolve, 100))
  assert.equal(runtime.messages.filter(message => message.type === 'run_released' && message.runId === nextRun).length, 0)
  assert.equal(runtime.messages.filter(message => message.requestId === prompt.id).length, 1)
})

test('CX-1 the sidecar cancels its Runs and exits when the Host closes stdin', async context => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-stdin-close-'))
  const runtime = startRuntime()
  registerRuntimeCleanup(context, runtime, directory)
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://stdin-close', modelId: 'stdin-close', apiType: 'faux',
    fauxResponses: [{ content: [{ type: 'toolCall', id: 'hang-tool', name: 'missing_native_tool', arguments: {} }], stopReason: 'toolUse' }],
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const identity = { conversationId: 'stdin-c', runtimeSessionId: 'stdin-s', runId: 'stdin-r' }
  const create = runtime.send('create_session', { ...identity, payload: { sessionFormat: 'pi-native', cwd: directory, sessionDir: join(directory, 'sessions') } })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  runtime.send('prompt', { ...identity, payload: { text: 'Wait on the Host.', controlBinding: nativeControlBinding(identity.conversationId, identity.runId) } })
  // The Host never answers the registration: the Run is parked on a Host request.
  await runtime.waitFor(message => message.type === 'tool_batch.register')
  const started = Date.now()
  runtime.child.stdin.end()
  await Promise.race([runtime.exited, new Promise((_, reject) => setTimeout(() => reject(new Error('sidecar did not exit after stdin closed')), 4000))])
  assert.ok(Date.now() - started < 4000)
  assert.equal(runtime.messages.filter(message => message.type === 'model_request.admit').length, 1, 'no new model request after the Host is gone')
})
