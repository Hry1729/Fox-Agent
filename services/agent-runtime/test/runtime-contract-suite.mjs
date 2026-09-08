import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { dirname } from 'node:path'
import { createEnvelope, PROTOCOL_NAME, PROTOCOL_VERSION } from '../src/protocol.mjs'
import { validateCapabilityManifest } from '../src/runtime-contract.mjs'

export function startRuntimeProcess(runtimePath) {
  const child = spawn(process.execPath, [runtimePath], { stdio: ['pipe', 'pipe', 'pipe'] })
  const messages = []
  const waiters = []
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  let stderr = ''
  child.stderr.on('data', (chunk) => { stderr += chunk.toString() })
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
        conversationId: request.conversationId ?? null,
        runtimeSessionId: request.runtimeSessionId ?? null,
        runId: request.runId ?? null,
        payload,
      })
      child.stdin.write(`${JSON.stringify(response)}\n`)
      return response
    },
    async waitFor(predicate, timeout = 5_000) {
      const existing = messages.find(predicate)
      if (existing) return existing
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          const index = waiters.indexOf(check)
          if (index >= 0) waiters.splice(index, 1)
          reject(new Error(`timed out waiting for runtime message; stderr=${stderr}; messages=${JSON.stringify(messages)}`))
        }, timeout)
        const check = () => {
          const message = messages.find(predicate)
          if (!message) return
          clearTimeout(timer)
          const index = waiters.indexOf(check)
          if (index >= 0) waiters.splice(index, 1)
          resolve(message)
        }
        waiters.push(check)
      })
    },
    close() {
      output.close()
      child.kill()
    },
  }
}

async function initializeRuntime(runtime, initializePayload) {
  const initialize = runtime.send('initialize', { payload: initializePayload })
  const ready = await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  assert.equal(ready.payload.protocol, PROTOCOL_NAME)
  assert.equal(ready.payload.protocolVersion, PROTOCOL_VERSION)
  assert.equal(typeof ready.payload.runtime, 'string')
  assert.ok(ready.payload.runtime)
  assert.equal(validateCapabilityManifest(ready.payload.capabilities), null)
  return ready.payload
}

async function createSession(runtime, { conversationId, runtimeSessionId, sessionPath }) {
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath },
  })
  const created = await runtime.waitFor((message) => message.requestId === create.id)
  assert.equal(created.type, 'session_created')
  assert.equal(created.payload.runtimeSessionId, runtimeSessionId)
}

function assertSingleTerminal(events, expectedType) {
  const terminals = events.filter(({ payload }) => ['run.completed', 'run.cancelled', 'run.failed', 'run.interrupted'].includes(payload.type))
  assert.equal(terminals.length, 1)
  assert.equal(terminals[0].payload.type, expectedType)
}

export async function runRuntimeAdapterContract({
  runtime,
  initializePayload,
  conversationId,
  runtimeSessionId,
  sessionPath,
  runId,
}) {
  const ready = await initializeRuntime(runtime, initializePayload)
  await createSession(runtime, { conversationId, runtimeSessionId, sessionPath })

  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { text: 'runtime adapter contract', messages: [{ role: 'user', content: 'runtime adapter contract' }] },
  })
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  const events = runtime.messages.filter((message) => message.runId === runId && message.type === 'runtime_event')
  assert.ok(events.length >= 4)
  assert.deepEqual(events.map(({ seq }) => seq), events.map(({ seq }) => seq).toSorted((left, right) => left - right))
  assert.equal(events[0].payload.type, 'run.started')
  assert.ok(events.some(({ payload }) => payload.type === 'message.started'))
  assert.ok(events.some(({ payload }) => payload.type === 'message.delta'))
  assert.ok(events.some(({ payload }) => payload.type === 'message.completed'))
  const usageEvents = events.filter(({ payload }) => payload.type === 'usage.updated')
  assert.ok(usageEvents.length >= 1)
  for (const { payload } of usageEvents) {
    for (const field of ['inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'totalTokens']) {
      assert.equal(Number.isSafeInteger(payload[field]) && payload[field] >= 0, true, field)
    }
    assert.ok(payload.totalTokens >= payload.inputTokens
      + payload.outputTokens
      + payload.cacheReadTokens
      + payload.cacheWriteTokens)
  }
  assertSingleTerminal(events, 'run.completed')
  return ready
}

export async function runCancellationContract({
  runtime,
  initializePayload,
  conversationId,
  runtimeSessionId,
  sessionPath,
  runId,
}) {
  const ready = await initializeRuntime(runtime, initializePayload)
  assert.equal(ready.capabilities.cancellation, true)
  await createSession(runtime, { conversationId, runtimeSessionId, sessionPath })
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { text: 'cancel this long response', messages: [{ role: 'user', content: 'cancel this long response' }] },
  })
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.started')
  const cancel = runtime.send('cancel', { conversationId, runtimeSessionId, runId })
  const accepted = await runtime.waitFor((message) => message.requestId === cancel.id)
  assert.equal(accepted.type, 'request_succeeded')
  assert.equal(accepted.payload.cancelling, true)
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.cancelled')
  assertSingleTerminal(runtime.messages.filter((message) => message.runId === runId && message.type === 'runtime_event'), 'run.cancelled')
}

export async function runSessionResumeContract({
  startRuntime,
  initializePayload,
  conversationId,
  runtimeSessionId,
  sessionPath,
}) {
  const first = startRuntime()
  try {
    const ready = await initializeRuntime(first, initializePayload)
    assert.equal(ready.capabilities.sessionResume, true)
    await createSession(first, { conversationId, runtimeSessionId, sessionPath })
    const runId = `${runtimeSessionId}-before-restart`
    first.send('prompt', {
      conversationId,
      runtimeSessionId,
      runId,
      payload: { text: 'persist this session', messages: [{ role: 'user', content: 'persist this session' }] },
    })
    await first.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
    await new Promise((resolve) => setTimeout(resolve, 100))
  } finally {
    first.close()
  }

  const resumed = startRuntime()
  try {
    await initializeRuntime(resumed, initializePayload)
    const request = resumed.send('resume_session', {
      conversationId,
      runtimeSessionId,
      payload: { sessionPath },
    })
    const response = await resumed.waitFor((message) => message.requestId === request.id)
    assert.equal(response.type, 'session_created')
    assert.equal(response.payload.runtimeSessionId, runtimeSessionId)
  } finally {
    resumed.close()
  }
}

export async function runToolApprovalContract({
  runtime,
  initializePayload,
  conversationId,
  runtimeSessionId,
  sessionPath,
  runId,
  expectedPath,
}) {
  const ready = await initializeRuntime(runtime, initializePayload)
  assert.equal(ready.capabilities.toolApproval, true)
  assert.ok(ready.capabilities.tools.some(({ approval }) => approval !== 'none'))
  await createSession(runtime, { conversationId, runtimeSessionId, sessionPath })
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { projectContext: { projectRoot: dirname(expectedPath) }, text: 'read the approved file', messages: [{ role: 'user', content: 'read the approved file' }] },
  })
  const preflight = await runtime.waitFor((message) => message.kind === 'request' && message.type === 'tool.preflight' && message.runId === runId)
  assert.equal(preflight.payload.tool, 'read')
  runtime.respond(preflight, 'tool.preflight.completed', {
    decision: 'allow',
    input: { ...preflight.payload.input, path: expectedPath },
  })
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'tool.completed')
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  const events = runtime.messages.filter((message) => message.runId === runId && message.type === 'runtime_event')
  const toolCompleted = events.find(({ payload }) => payload.type === 'tool.completed')
  assert.equal(toolCompleted.payload.tool, 'read')
  assert.equal(preflight.payload.toolCallId, toolCompleted.payload.toolCallId)
  assert.equal(toolCompleted.payload.isError, false)
  assertSingleTerminal(events, 'run.completed')
}

export async function runCapabilityDegradationContract({
  runtime,
  initializePayload,
  conversationId,
  runtimeSessionId,
  sessionPath,
  runId,
}) {
  const ready = await initializeRuntime(runtime, initializePayload)
  assert.equal(ready.capabilities.reasoning, false)
  assert.equal(ready.capabilities.toolApproval, false)
  assert.equal(ready.capabilities.imageInput, false)
  assert.deepEqual(ready.capabilities.tools, [])
  await createSession(runtime, { conversationId, runtimeSessionId, sessionPath })
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { text: 'degraded capability contract', messages: [{ role: 'user', content: 'degraded capability contract' }] },
  })
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  const events = runtime.messages.filter((message) => message.runId === runId && message.type === 'runtime_event')
  assert.equal(events.some(({ payload }) => payload.type.startsWith('reasoning.')), false)
  assert.equal(events.some(({ payload }) => payload.type.startsWith('tool.')), false)
  assertSingleTerminal(events, 'run.completed')
}
