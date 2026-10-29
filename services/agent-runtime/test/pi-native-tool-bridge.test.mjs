// Repo form of the S0 probes P1-P5, driving the Pi-native tool bridge against a
// faux provider and a stub Host that implements the same barrier rule the Rust
// `native_tool_batch` must enforce: a leading run of read-only calls may
// overlap (at most 4 at once), and any write or unknown tool is a barrier that
// waits for every earlier position to settle.
import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { Agent } from '@earendil-works/pi-agent-core'
import { createFauxCore, fauxAssistantMessage, fauxToolCall } from '@earendil-works/pi-ai'
import { Type } from 'typebox'
import {
  createNativeToolBridge,
  TOOL_BATCH_REGISTER,
  TOOL_CALL_EXECUTE,
  TOOL_CALL_CLOSE,
  TOOL_CALL_CANCEL,
} from '../src/pi-native-tool-bridge.mjs'

const READ_ONLY = new Set(['read', 'grep', 'ls', 'find', 'skill_load'])
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function fauxCore(name) {
  return createFauxCore({
    api: `faux-${name}`,
    provider: `faux-${name}`,
    models: [{ id: 'faux-1', name: 'Faux 1', contextWindow: 128000, maxTokens: 4096 }],
  })
}

function toolDef(name, execute) {
  return {
    name,
    label: name,
    description: `probe tool ${name}`,
    parameters: Type.Object({ path: Type.Optional(Type.String()) }),
    execute,
  }
}

/** Stub Host: durable-ish batch ledger plus the source-ordered barrier. */
function createStubHost({ readLimit = 4, delayMs = {} } = {}) {
  const requests = []
  const batches = new Map()
  const windows = []
  let sequence = 0

  const running = (batch) => batch.calls.filter((c) => c.state === 'running').length

  // Admission is a single synchronous step: checking the barrier and claiming
  // the slot must not be separated by an await, or a whole read-only prefix
  // admits itself at once and the limit is meaningless.
  function tryAdmit(batch, call) {
    const isRead = READ_ONLY.has(call.tool)
    const blocked = batch.calls.some((c) => {
      if (c.sourceOrder >= call.sourceOrder || c.settled) return false
      // A barrier waits for every position ahead of it; a read-only position
      // only waits for an earlier barrier, and is capped by the read limit.
      return isRead ? !READ_ONLY.has(c.tool) : true
    })
    if (blocked) return false
    if (isRead && running(batch) >= readLimit) return false
    call.state = 'running'
    return true
  }

  async function waitTurn(batch, call, signal) {
    const deadline = AbortSignal.timeout(2000)
    while (!tryAdmit(batch, call)) {
      signal?.throwIfAborted()
      deadline.throwIfAborted()
      await sleep(2)
    }
  }

  return {
    requests,
    windows,
    batches,
    get batchCount() { return batches.size },
    async request(type, payload, signal) {
      signal?.throwIfAborted()
      requests.push({ type, payload })
      if (type === TOOL_BATCH_REGISTER) {
        const existing = [...batches.values()].find(
          (b) => b.runId === payload.runId
            && b.generation === payload.contextGeneration
            && b.calls.length === payload.calls.length
            && b.calls.every((c, i) => c.callId === payload.calls[i].callId),
        )
        if (existing) return { payload: { batchId: existing.batchId, registeredCalls: existing.calls.length } }
        const batchId = `batch-${++sequence}`
        batches.set(batchId, {
          batchId,
          runId: payload.runId,
          generation: payload.contextGeneration,
          calls: payload.calls.map((c) => ({ ...c, state: 'pending', settled: false, result: null })),
        })
        return { payload: { batchId, registeredCalls: payload.calls.length } }
      }

      const batch = batches.get(payload.batchId)
      if (!batch) throw new Error(`unknown batch ${payload.batchId}`)
      const call = batch.calls.find((c) => c.callId === payload.callId)
      if (!call) throw new Error(`unknown call ${payload.callId}`)

      if (type === TOOL_CALL_CLOSE) {
        if (call.settled) return { payload: { closed: false, state: call.state } }
        call.state = payload.reason === 'not_started' ? 'not_started' : 'cancelled'
        call.settled = true
        call.isError = payload.isError
        return { payload: { closed: true, state: call.state } }
      }
      if (type === TOOL_CALL_CANCEL) {
        const recorded = call.state === 'running' || call.settled
        call.state = 'cancelled'
        call.settled = true
        return { payload: { cancelled: true, resultRecorded: recorded, outcomeUnknown: !recorded } }
      }
      if (type !== TOOL_CALL_EXECUTE) throw new Error(`unexpected request ${type}`)

      if (call.settled) {
        // A replayed claim must not execute twice.
        return call.result
          ? { payload: call.result }
          : { payload: { isError: true, error: `call ${call.callId} is already settled as ${call.state}` } }
      }
      await waitTurn(batch, call, signal)
      const startAt = performance.now()
      await sleep(delayMs[call.tool] ?? 0)
      const result = { payload: { result: { content: [{ type: 'text', text: `${call.tool}:${call.callId}` }] } } }
      windows.push({ tool: call.tool, callId: call.callId, startAt, endAt: performance.now() })
      call.state = 'completed'
      call.settled = true
      call.result = result.payload
      return result
    },
  }
}

function harness(name, { calls, delayMs = {}, readLimit = 4, tools, requestHost, beforeToolCall, awaitEvents = false }) {
  const host = createStubHost({ readLimit, delayMs })
  const bridge = createNativeToolBridge({
    requestHost: (type, payload, signal) => requestHost
      ? requestHost(type, payload, signal, host)
      : host.request(type, payload, signal),
    runId: `run-${name}`,
    contextGeneration: 4,
  })
  const events = []
  const eventErrors = []
  const core = fauxCore(name)
  core.setResponses([
    fauxAssistantMessage(
      calls.map((call) => fauxToolCall(call.name, call.args ?? {}, { id: call.id })),
      { stopReason: 'toolUse' },
    ),
    fauxAssistantMessage('done', { stopReason: 'stop' }),
  ])
  const agent = new Agent({
    initialState: {
      systemPrompt: 'bridge probe',
      model: core.getModel(),
      tools: bridge.wrapTools(tools),
      thinkingLevel: 'off',
    },
    streamFn: core.streamSimple,
    beforeToolCall: async (context, signal) => {
      const result = await bridge.beforeToolCall(context, signal)
      return beforeToolCall ? beforeToolCall(context, signal) : result
    },
  })
  agent.subscribe((event, signal) => {
    events.push(event)
    const completion = bridge.onEvent(event, signal)
    if (awaitEvents) return completion
    completion.catch((error) => eventErrors.push(error))
  })
  return { host, bridge, agent, events, eventErrors }
}

function deferred() {
  let resolve
  const promise = new Promise((yes) => { resolve = yes })
  return { promise, resolve }
}

async function within(promise) {
  let timer
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('test wait timed out')), 2000) }),
    ])
  } finally {
    clearTimeout(timer)
  }
}

const overlapping = (a, b) => a.startAt < b.endAt && b.startAt < a.endAt
const byCall = (windows) => Object.fromEntries(windows.map((w) => [w.callId, w]))

// Mirrors `is_parallel_read_only_tool` in runtime_host/native_tool_batch.rs.
// The Host owns the barrier, so if these two lists drift the JS side would mark
// a tool parallel that the Host treats as a barrier, or the reverse.
const HOST_READ_ONLY_WHITELIST = ['read', 'ls', 'find', 'grep', 'skill_load']

test('the read-only whitelist names only Fox-managed tools', async () => {
  const { RUNTIME_TOOL_CATALOG } = await import('../../../packages/fox-engine-protocol/index.mjs')
  const { createReadOnlyTools } = await import('../src/read-only-tools.mjs')
  const catalogNames = new Set(RUNTIME_TOOL_CATALOG.map((entry) => entry.name))
  const readOnlyNames = new Set(createReadOnlyTools(async () => ({ payload: { result: {} } })).map((t) => t.name))

  for (const name of HOST_READ_ONLY_WHITELIST) {
    assert.ok(catalogNames.has(name), `${name} must be in the frozen runtime tool catalog`)
    // skill_load is a host tool, the other four come from the read-only factory;
    // either way the name must resolve to a Fox tool, never to an SDK built-in
    // file or shell executor.
    assert.ok(readOnlyNames.has(name) || name === 'skill_load',
      `${name} must be provided by Fox, not by a Pi built-in executor`)
  }
  assert.equal(RUNTIME_TOOL_CATALOG.length >= HOST_READ_ONLY_WHITELIST.length, true)
})

test('every managed tool is marked parallel and the batch registers once', async () => {
  const { host, bridge, agent } = harness('register', {
    calls: [{ id: 'c1', name: 'read' }, { id: 'c2', name: 'write_file' }],
    tools: [
      toolDef('read', async (id, params) => ({ content: [{ type: 'text', text: 'ok' }] })),
      toolDef('write_file', async () => ({ content: [{ type: 'text', text: 'ok' }] })),
    ],
  })
  const wrapped = bridge.wrapTools([toolDef('probe', async () => ({}))])
  assert.equal(wrapped[0].executionMode, 'parallel')

  await agent.prompt('go')
  await agent.waitForIdle()
  await bridge.drain()

  const registers = host.requests.filter((r) => r.type === TOOL_BATCH_REGISTER)
  assert.equal(registers.length, 1, 'one batch must produce exactly one registration')
  assert.equal(registers[0].payload.contextGeneration, 4)
  assert.equal(registers[0].payload.runId, 'run-register')
  assert.deepEqual(
    registers[0].payload.calls.map((c) => [c.callId, c.tool, c.sourceOrder]),
    [['c1', 'read', 0], ['c2', 'write_file', 1]],
  )
  for (const call of registers[0].payload.calls) {
    assert.match(call.inputDigest, /^sha256:[0-9a-f]{64}$/, 'the model arguments must be digested')
  }
  assert.equal(host.requests.filter((r) => r.type === TOOL_CALL_EXECUTE).length, 2)
})

test('beforeToolCall waits for registration only, not early close or execution settlement', async (t) => {
  const registering = deferred()
  const registered = deferred()
  const closing = deferred()
  const closed = deferred()
  const prepared = deferred()
  const executing = deferred()
  const executed = deferred()
  let preparedCount = 0
  t.after(() => { registered.resolve(); closed.resolve(); executed.resolve() })
  const { host, bridge, agent, events } = harness('noreap', {
    calls: [
      { id: 'ghost', name: 'missing' },
      { id: 'read', name: 'read' },
      { id: 'write', name: 'write_file' },
    ],
    tools: [toolDef('read'), toolDef('write_file')],
    beforeToolCall: () => { if (++preparedCount === 2) prepared.resolve() },
    requestHost: async (type, payload, signal, host) => {
      if (type === TOOL_BATCH_REGISTER) {
        registering.resolve()
        await registered.promise
      }
      if (type === TOOL_CALL_CLOSE) {
        closing.resolve()
        await closed.promise
      }
      if (type === TOOL_CALL_EXECUTE) {
        executing.resolve()
        await executed.promise
      }
      return host.request(type, payload, signal)
    },
  })
  const prompt = agent.prompt('go')
  await within(registering.promise)
  assert.equal(preparedCount, 0, 'preparation must wait for the registration acknowledgement')
  assert.equal(host.requests.length, 0)
  let closeResolved = false
  const pendingClose = bridge.onEvent({ type: 'tool_execution_end', toolCallId: 'ghost', toolName: 'missing',
    isError: true, result: { content: [{ type: 'text', text: 'Tool missing not found' }], details: {} } })
    .then(() => { closeResolved = true })
  await Promise.resolve()
  assert.equal(closeResolved, false, 'an event during registration must not be dropped')
  registered.resolve()
  await within(prepared.promise)
  await within(closing.promise)
  await within(executing.promise)
  assert.equal(preparedCount, 2, 'all valid calls prepare before any settlement')
  assert.equal(host.requests.filter((r) => r.type === TOOL_CALL_CLOSE).length, 0)
  assert.equal(host.windows.length, 0)
  closed.resolve()
  executed.resolve()
  await within(prompt)
  await pendingClose
  await bridge.drain()
  assert.equal(host.requests.filter((r) => r.type === TOOL_CALL_CLOSE).length, 1)
  assert.equal(host.windows.length, 2)
  assert.equal(bridge.pendingPositions, 0)
})

test('a mixed batch keeps the read-only prefix parallel and the write a barrier', async () => {
  const { host, bridge, agent } = harness('barrier', {
    calls: [
      { id: 'A', name: 'read' },
      { id: 'B', name: 'grep' },
      { id: 'C', name: 'write_file' },
      { id: 'D', name: 'read' },
    ],
    delayMs: { read: 60, grep: 60, write_file: 10 },
    tools: [
      toolDef('read', async () => ({ content: [{ type: 'text', text: 'ok' }] })),
      toolDef('grep', async () => ({ content: [{ type: 'text', text: 'ok' }] })),
      toolDef('write_file', async () => ({ content: [{ type: 'text', text: 'ok' }] })),
    ],
  })
  await agent.prompt('go')
  await agent.waitForIdle()
  await bridge.drain()

  const w = byCall(host.windows)
  assert.ok(w.A && w.B && w.C && w.D, `all four positions executed: ${JSON.stringify(Object.keys(w))}`)
  assert.ok(overlapping(w.A, w.B), 'the two leading reads must overlap')
  assert.ok(w.C.startAt >= Math.max(w.A.endAt, w.B.endAt), 'the write must wait for both reads')
  assert.ok(w.D.startAt >= w.C.endAt, 'the read after a write must wait for it')
  assert.equal(host.batchCount, 1)
})

test('the read-only prefix never exceeds the concurrency limit', async () => {
  const calls = Array.from({ length: 6 }, (_, i) => ({ id: `r${i}`, name: 'read' }))
  const { host, bridge, agent } = harness('cap', {
    calls,
    delayMs: { read: 40 },
    readLimit: 4,
    tools: [toolDef('read', async () => ({ content: [{ type: 'text', text: 'ok' }] }))],
  })
  await agent.prompt('go')
  await agent.waitForIdle()
  await bridge.drain()

  const points = host.windows.flatMap((w) => [[w.startAt, 1], [w.endAt, -1]])
  points.sort((a, b) => a[0] - b[0] || a[1] - b[1])
  let peak = 0
  let current = 0
  for (const [, delta] of points) { current += delta; peak = Math.max(peak, current) }
  assert.ok(peak <= 4, `peak concurrency was ${peak}`)
  assert.equal(host.windows.length, 6)
})

test('a position that never executes is closed with an honest reason', async () => {
  const { host, bridge, agent } = harness('close', {
    calls: [
      { id: 'known', name: 'read' },
      { id: 'ghost', name: 'not_a_real_tool' },
    ],
    tools: [toolDef('read', async () => ({ content: [{ type: 'text', text: 'ok' }] }))],
  })
  await agent.prompt('go')
  await agent.waitForIdle()
  await bridge.drain()

  const closes = host.requests.filter((r) => r.type === TOOL_CALL_CLOSE)
  const ghost = closes.find((r) => r.payload.callId === 'ghost')
  assert.ok(ghost, `the unexecutable position must be closed; saw ${JSON.stringify(closes.map((c) => c.payload))}`)
  assert.equal(ghost.payload.isError, true, 'closing a position is never a success')
  assert.ok(
    ['unknown_tool', 'validation_failed'].includes(ghost.payload.reason),
    `unexpected close reason ${ghost.payload.reason}`,
  )
  assert.ok(
    host.requests.some((r) => r.type === TOOL_CALL_EXECUTE && r.payload.callId === 'known'),
    'the executable position still ran',
  )
})

for (const awaitEvents of [false, true]) {
  for (const invalid of [false, true]) {
    test(`early ${invalid ? 'invalid' : 'unknown'} position releases the barrier (await events: ${awaitEvents})`, async () => {
      const reason = invalid ? 'validation_failed' : 'unknown_tool'
      const { host, bridge, agent, events } = harness(`early-${reason}-${awaitEvents}`, {
        calls: [
          { id: 'bad', name: invalid ? 'write_file' : 'missing', args: { path: {} } },
          { id: 'read', name: 'read' },
          { id: 'write', name: 'write_file' },
        ],
        tools: [toolDef('read'), toolDef('write_file')],
        awaitEvents,
        requestHost: (type, payload, signal, host) => {
          if (type === TOOL_BATCH_REGISTER) {
            assert.ok(events.some((e) => e.type === 'message_end' && e.message?.role === 'assistant'))
            assert.ok(!events.some((e) => e.type === 'tool_execution_end' && e.toolCallId === 'bad'),
              'registration must precede even an SDK-rejected position')
          }
          return host.request(type, payload, signal)
        },
      })
      await within(agent.prompt('go'))
      await bridge.drain()
      assert.deepEqual(host.requests.filter((r) => r.type === TOOL_CALL_CLOSE).map((r) => [r.payload.callId, r.payload.reason]),
        [['bad', reason]])
      assert.equal(host.windows.length, 2)
      assert.equal(bridge.pendingPositions, 0)
      assert.equal(events.find((e) => e.type === 'tool_execution_end' && e.toolCallId === 'bad').isError, true)
      assert.ok(events.filter((e) => e.type === 'tool_execution_end' && e.toolCallId !== 'bad').every((e) => !e.isError))
    })
  }

  test(`an entirely invalid SDK batch registers from assistant message_end (await events: ${awaitEvents})`, async () => {
    let preparations = 0
    const { host, bridge, agent, events } = harness(`all-invalid-${awaitEvents}`, {
      calls: [
        { id: 'ghost', name: 'missing' },
        { id: 'invalid', name: 'write_file', args: { path: {} } },
      ],
      tools: [toolDef('write_file')],
      awaitEvents,
      beforeToolCall: () => { preparations += 1 },
      requestHost: (type, payload, signal, host) => {
        if (type === TOOL_BATCH_REGISTER) {
          const assistant = events.find((e) => e.type === 'message_end' && e.message?.role === 'assistant')
          assert.ok(assistant)
          assert.ok(!events.some((e) => e.type === 'turn_end'))
          assert.deepEqual(assistant.message.content.filter((c) => c.type === 'toolCall').map((c) => c.id), ['ghost', 'invalid'])
        }
        return host.request(type, payload, signal)
      },
    })
    await within(agent.prompt('go'))
    await bridge.drain()
    assert.equal(preparations, 0)
    assert.equal(host.requests.filter((r) => r.type === TOOL_BATCH_REGISTER).length, 1)
    assert.deepEqual(host.requests.filter((r) => r.type === TOOL_CALL_CLOSE).map((r) => [r.payload.callId, r.payload.reason]),
      [['ghost', 'unknown_tool'], ['invalid', 'validation_failed']])
    assert.equal(host.windows.length, 0)
    assert.equal(bridge.pendingPositions, 0)
  })
}

for (const failure of ['transport', 'malformed', 'unconfirmed', 'lost-ack']) {
  test(`a ${failure} close failure propagates and duplicate SDK events can retry`, async (t) => {
    const firstResponse = deferred()
    const retryStarted = deferred()
    const retryResponse = deferred()
    let attempts = 0
    t.after(() => { firstResponse.resolve(); retryResponse.resolve() })
    const expected = failure === 'malformed' ? /invalid tool_call.close response/
      : failure === 'unconfirmed' ? /close was not confirmed/ : /close transport failed/
    const { host, bridge, agent, events, eventErrors } = harness(`close-${failure}`, {
      calls: [{ id: 'ghost', name: 'missing' }],
      tools: [toolDef('read')],
      requestHost: async (type, payload, signal, host) => {
        if (type === TOOL_CALL_CLOSE) {
          if (++attempts === 1) {
            await firstResponse.promise
            if (failure === 'malformed') return { payload: {} }
            if (failure === 'unconfirmed') return { payload: { closed: false, state: 'pending' } }
            if (failure === 'lost-ack') await host.request(type, payload, signal)
            throw new Error('close transport failed')
          }
          retryStarted.resolve()
          await retryResponse.promise
        }
        return host.request(type, payload, signal)
      },
    })
    await within(agent.prompt('go'))
    assert.equal(attempts, 1, 'turn_end duplicates share the in-flight close')
    assert.equal(bridge.pendingPositions, 1, 'an unacknowledged close is still pending')
    const end = events.find((e) => e.type === 'tool_execution_end')
    const rejectedEvent = assert.rejects(bridge.onEvent(end), expected)
    const rejectedDrain = assert.rejects(bridge.drain(), expected)
    firstResponse.resolve()
    await Promise.all([rejectedEvent, rejectedDrain])
    assert.equal(bridge.pendingPositions, 1)
    assert.ok(eventErrors.length > 0, 'turn_end also propagates the close failure')
    for (const error of eventErrors) assert.match(error.message, expected)

    const retry = bridge.onEvent(end)
    const duplicate = bridge.onEvent(end)
    assert.equal(retry, duplicate, 'duplicate events share the same close promise')
    await within(retryStarted.promise)
    assert.equal(attempts, 2)
    assert.equal(bridge.pendingPositions, 1)
    let drained = false
    const drainedPromise = bridge.drain().then(() => { drained = true })
    await Promise.resolve()
    assert.equal(drained, false, 'drain waits for the close acknowledgement')
    retryResponse.resolve()
    await Promise.all([retry, duplicate, drainedPromise])
    assert.equal(bridge.pendingPositions, 0)
    await bridge.onEvent(end)
    await bridge.onEvent(events.find((e) => e.type === 'turn_end'))
    await bridge.drain()
    assert.equal(attempts, 2, 'acknowledged positions are not closed again')
    assert.ok([...host.batches.values()][0].calls[0].settled)
  })
}

for (const [returnsSuccess, cancelFails] of [[false, false], [true, false], [true, true]]) {
  test(`aborted SDK execution: transport success=${returnsSuccess}, cancel failure=${cancelFails}`, async (t) => {
    const started = deferred()
    const response = deferred()
    let executionSignal
    const cleanupSignals = []
    t.after(() => response.resolve())
    const { host, bridge, agent, events } = harness(`abort-${returnsSuccess}`, {
      calls: [{ id: 'slow', name: 'read' }],
      tools: [toolDef('read')],
      requestHost: async (type, payload, signal, host) => {
        signal.throwIfAborted()
        if (type === TOOL_CALL_EXECUTE) {
          executionSignal = signal
          const result = await host.request(type, payload, signal)
          started.resolve()
          await response.promise
          if (!returnsSuccess) signal.throwIfAborted()
          return result
        }
        if (type === TOOL_CALL_CANCEL) {
          cleanupSignals.push(signal)
          if (cancelFails) throw new Error('cancel transport failed')
        }
        return host.request(type, payload, signal)
      },
    })
    const prompt = agent.prompt('go')
    await within(started.promise)
    agent.abort()
    response.resolve()
    await within(prompt)
    await bridge.drain()
    assert.equal(executionSignal.aborted, true)
    assert.equal(cleanupSignals.length, 1)
    assert.notEqual(cleanupSignals[0], executionSignal)
    assert.equal(cleanupSignals[0].aborted, false)
    assert.equal(host.requests.filter((r) => r.type === TOOL_CALL_CANCEL).length, cancelFails ? 0 : 1)
    if (!cancelFails) assert.equal([...host.batches.values()][0].calls[0].state, 'cancelled')
    const end = events.find((e) => e.type === 'tool_execution_end' && e.toolCallId === 'slow')
    assert.equal(end.isError, true)
    assert.match(end.result.content[0].text, cancelFails ? /cancel transport failed/ : /abort/i)
    assert.equal(agent.state.messages.find((m) => m.role === 'toolResult').isError, true)
  })
}

for (const cancelFails of [false, true]) {
  test(`execute already aborted uses bounded cleanup and ${cancelFails ? 'propagates cancellation failure' : 'never submits execution'}`, async (t) => {
    const host = createStubHost()
    const controller = new AbortController()
    const timeouts = []
    const timeout = AbortSignal.timeout.bind(AbortSignal)
    t.mock.method(AbortSignal, 'timeout', (ms) => { timeouts.push(ms); return timeout(ms) })
    const cleanupFailure = new Error('cancel transport failed')
    const bridge = createNativeToolBridge({
      runId: `pre-abort-${cancelFails}`,
      contextGeneration: 1,
      requestHost: (type, payload, signal) => {
        signal?.throwIfAborted()
        if (type === TOOL_CALL_CANCEL) {
          assert.notEqual(signal, controller.signal)
          if (cancelFails) throw cleanupFailure
        }
        return host.request(type, payload, signal)
      },
    })
    const [tool] = bridge.wrapTools([toolDef('read')])
    const toolCall = fauxToolCall('read', {}, { id: 'call' })
    await bridge.beforeToolCall({ assistantMessage: fauxAssistantMessage([toolCall]), toolCall }, controller.signal)
    controller.abort()
    await assert.rejects(tool.execute('call', {}, controller.signal), cancelFails ? cleanupFailure : /aborted before execution/)
    assert.deepEqual(timeouts, [5000])
    assert.equal(host.requests.filter((r) => r.type === TOOL_CALL_EXECUTE).length, 0)
    assert.equal(host.requests.filter((r) => r.type === TOOL_CALL_CANCEL).length, cancelFails ? 0 : 1)
  })
}

test('an aborted SDK turn registers and closes positions that never reached preparation', async () => {
  const signals = []
  const { host, bridge, agent } = harness('abort-preparation', {
    calls: [{ id: 'ghost', name: 'missing' }, { id: 'unstarted', name: 'read' }],
    tools: [toolDef('read')],
    requestHost: (type, payload, signal, host) => {
      signal.throwIfAborted()
      signals.push(signal)
      return host.request(type, payload, signal)
    },
  })
  let runSignal
  agent.subscribe((event, signal) => {
    if (event.type === 'tool_execution_end' && event.toolCallId === 'ghost') {
      runSignal = signal
      agent.abort()
    }
  })
  await within(agent.prompt('go'))
  await bridge.drain()
  assert.equal(runSignal.aborted, true)
  assert.equal(signals.length, 3)
  assert.ok(signals.every((signal) => signal !== runSignal && !signal.aborted))
  assert.deepEqual(host.requests.filter((r) => r.type === TOOL_CALL_CLOSE).map((r) => [r.payload.callId, r.payload.reason]),
    [['ghost', 'unknown_tool'], ['unstarted', 'not_started']])
  assert.equal(bridge.pendingPositions, 0)
})

test('an empty tool catalog still closes an all-unknown SDK batch', async () => {
  const { host, bridge, agent } = harness('empty-catalog', {
    calls: [{ id: 'ghost', name: 'missing' }],
    tools: [],
  })
  await agent.prompt('go')
  await bridge.drain()
  assert.equal(host.requests.filter((r) => r.type === TOOL_BATCH_REGISTER).length, 1)
  assert.deepEqual(host.requests.filter((r) => r.type === TOOL_CALL_CLOSE).map((r) => r.payload.reason), ['unknown_tool'])
  assert.equal(bridge.pendingPositions, 0)
})

test('raw argument digests are stable nested JSON, not SDK-prepared input', async () => {
  const variants = [
    { z: [{ b: 2, a: 1 }, '2'], count: '7', a: { y: 2, x: 1 } },
    { a: { x: 1, y: 2 }, count: '7', z: [{ a: 1, b: 2 }, '2'] },
    { a: { x: 1, y: 2 }, count: '7', z: ['2', { a: 1, b: 2 }] },
  ]
  const digests = []
  for (const [index, args] of variants.entries()) {
    const { host, bridge, agent } = harness(`digest-${index}`, {
      calls: [{ id: 'call', name: 'read', args }],
      tools: [{
        ...toolDef('read'),
        parameters: Type.Object({ count: Type.Number(), a: Type.Any(), z: Type.Array(Type.Any()) }),
        prepareArguments: (raw) => ({ ...raw, count: Number(raw.count) }),
      }],
    })
    await agent.prompt('go')
    await bridge.drain()
    digests.push(host.requests.find((r) => r.type === TOOL_BATCH_REGISTER).payload.calls[0].inputDigest)
    assert.equal(host.requests.find((r) => r.type === TOOL_CALL_EXECUTE).payload.input.count, 7)
    assert.equal(args.count, '7')
  }
  const expected = 'sha256:' + createHash('sha256')
    .update('{"a":{"x":1,"y":2},"count":"7","z":[{"a":1,"b":2},"2"]}').digest('hex')
  assert.equal(digests[0], expected)
  assert.equal(digests[1], expected)
  assert.notEqual(digests[2], expected, 'array order remains part of the raw input identity')
})
