import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { mkdtemp, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'

const runtimePath = fileURLToPath(new URL('../src/fake-runtime.mjs', import.meta.url))

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
    async waitFor(predicate, timeout = 2_000) {
      const found = messages.find(predicate)
      if (found) return found

      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          const index = waiters.indexOf(check)
          if (index >= 0) waiters.splice(index, 1)
          reject(new Error(`timed out waiting for runtime message: ${JSON.stringify(messages)}`))
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

test('streams a prompt through the JSONL process boundary', async (context) => {
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize')
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const conversationId = 'conversation-1'
  const runtimeSessionId = 'session-1'
  const createSession = runtime.send('create_session', { conversationId, runtimeSessionId })
  await runtime.waitFor((message) => message.requestId === createSession.id && message.type === 'session_created')

  const runId = 'run-1'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { text: 'hello runtime' },
  })
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'message.delta')
  const completed = await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
  assert.ok(completed.seq > 1)
})

test('cancels an active run before completion', async (context) => {
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize')
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  runtime.send('create_session', { conversationId: 'conversation-2', runtimeSessionId: 'session-2' })

  runtime.send('prompt', {
    conversationId: 'conversation-2',
    runtimeSessionId: 'session-2',
    runId: 'run-2',
    payload: { text: 'please produce a response that takes long enough to cancel' },
  })
  await runtime.waitFor((message) => message.runId === 'run-2' && message.payload?.type === 'message.delta')
  runtime.send('cancel', { conversationId: 'conversation-2', runtimeSessionId: 'session-2', runId: 'run-2' })
  const cancelled = await runtime.waitFor((message) => message.runId === 'run-2' && message.payload?.type === 'run.cancelled')
  assert.equal(cancelled.payload.type, 'run.cancelled')
})

test('resumes a session after the runtime process restarts', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-runtime-session-'))
  const sessionPath = join(directory, 'session.json')
  context.after(() => rm(directory, { recursive: true, force: true }))

  const firstRuntime = startRuntime()
  const initialize = firstRuntime.send('initialize')
  await firstRuntime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  const created = firstRuntime.send('create_session', {
    conversationId: 'conversation-resume',
    runtimeSessionId: 'session-resume',
    payload: { sessionPath },
  })
  await firstRuntime.waitFor((message) => message.requestId === created.id && message.type === 'session_created')
  firstRuntime.close()

  const secondRuntime = startRuntime()
  context.after(() => secondRuntime.close())
  const secondInitialize = secondRuntime.send('initialize')
  await secondRuntime.waitFor((message) => message.requestId === secondInitialize.id && message.type === 'ready')
  const resumed = secondRuntime.send('resume_session', {
    conversationId: 'conversation-resume',
    runtimeSessionId: 'session-resume',
    payload: { sessionPath },
  })
  const response = await secondRuntime.waitFor((message) => message.requestId === resumed.id)
  assert.equal(response.type, 'session_created')
})

test('pauses a tool until the host returns a preflight decision', async (context) => {
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize')
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  runtime.send('create_session', { conversationId: 'conversation-tool', runtimeSessionId: 'session-tool' })
  runtime.send('prompt', {
    conversationId: 'conversation-tool',
    runtimeSessionId: 'session-tool',
    runId: 'run-tool',
    payload: {
      text: 'probe a read-only tool',
      toolProbe: { tool: 'read', input: { path: 'README.md' } },
    },
  })

  const preflight = await runtime.waitFor((message) => message.kind === 'request' && message.type === 'tool.preflight')
  assert.equal(preflight.payload.tool, 'read')
  runtime.respond(preflight, 'tool.preflight_allowed', {
    decision: 'allow',
    tool: 'read',
    input: { path: 'D:/project/README.md' },
  })
  const completed = await runtime.waitFor((message) => message.runId === 'run-tool' && message.payload?.type === 'tool.completed')
  assert.equal(completed.payload.decision, 'allow')
})

test('exercises the A0 work loop through tool.execute and emits a versioned event', async (context) => {
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize', { payload: { workLoop: true } })
  const ready = await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  assert.equal(ready.payload.capabilities.workLoop, true)
  assert.ok(ready.payload.capabilities.tools.some(({ name }) => name === 'goal_propose'))

  runtime.send('create_session', { conversationId: 'conversation-work', runtimeSessionId: 'session-work' })
  runtime.send('prompt', {
    conversationId: 'conversation-work',
    runtimeSessionId: 'session-work',
    runId: 'run-work',
    payload: {
      text: 'exercise the work loop',
      workLoopProbe: {
        tool: 'goal_propose',
        input: { title: 'A0', objective: 'Close the loop' },
        sequence: 4,
      },
    },
  })

  const execute = await runtime.waitFor((message) => message.kind === 'request' && message.type === 'tool.execute')
  runtime.respond(execute, 'tool.execute_completed', {
    isError: false,
    result: {
      content: [{ type: 'text', text: 'Goal proposed' }],
      details: { goal: { id: 'goal-work', title: 'A0' } },
    },
  })
  const event = await runtime.waitFor((message) => message.payload?.type === 'goal.proposed')
  assert.equal(event.payload.schemaVersion, 1)
  assert.equal(event.payload.conversationId, 'conversation-work')
  assert.equal(event.payload.goalId, 'goal-work')
  assert.equal(event.payload.sequence, 4)
})

test('rejects unsupported profiles and blocks fake Host mutations in shadow mode', async (context) => {
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const invalid = runtime.send('initialize', {
    payload: { executionProfile: 'durable_v2', executionStrategy: { validationPolicy: 'legacy' } },
  })
  const rejected = await runtime.waitFor((message) => message.requestId === invalid.id)
  assert.equal(rejected.type, 'request_failed')
  assert.equal(rejected.payload.code, 'runtime.execution_profile.invalid')

  const initialize = runtime.send('initialize', {
    payload: { workLoop: true, executionProfile: 'durable_v2_shadow' },
  })
  const ready = await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  assert.equal(ready.payload.executionProfile.id, 'durable_v2_shadow')
  assert.ok(!ready.payload.capabilities.tools.some(({ name }) => name === 'goal_propose'))

  runtime.send('create_session', { conversationId: 'conversation-shadow', runtimeSessionId: 'session-shadow' })
  runtime.send('prompt', {
    conversationId: 'conversation-shadow',
    runtimeSessionId: 'session-shadow',
    runId: 'run-shadow',
    payload: {
      text: 'shadow probe',
      workLoopProbe: { tool: 'goal_propose', input: { title: 'must not persist' } },
    },
  })
  const blocked = await runtime.waitFor((message) => (
    message.runId === 'run-shadow'
    && message.payload?.type === 'tool.completed'
    && message.payload?.code === 'runtime.execution_profile.tool_blocked'
  ))
  assert.equal(blocked.payload.isError, true)
  assert.equal(runtime.messages.some((message) => (
    message.kind === 'request' && message.type === 'tool.execute' && message.runId === 'run-shadow'
  )), false)
  await runtime.waitFor((message) => message.runId === 'run-shadow' && message.payload?.type === 'run.completed')
})
