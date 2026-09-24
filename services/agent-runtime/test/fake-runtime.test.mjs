import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { performance } from 'node:perf_hooks'
import { createEnvelope } from '../src/protocol.mjs'

const runtimePath = fileURLToPath(new URL('../src/fake-runtime.mjs', import.meta.url))
const MESSAGE_TIMEOUT_MS = 2_000
const STARTUP_READY_TIMEOUT_MS = 10_000
const STDERR_LIMIT = 4_096 // Tail length in JavaScript string code units.

function startRuntime(scriptPath = runtimePath) {
  const startedAt = performance.now()
  const child = spawn(process.execPath, [scriptPath], { stdio: ['pipe', 'pipe', 'pipe'] })
  const messages = []
  const waiters = []
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  const state = {
    spawnMs: null, readyMs: null, exit: null, closed: false,
    spawnError: null, stdinError: null, stdoutError: null, protocolError: null, stderr: '',
  }
  const elapsedMs = () => Math.round(performance.now() - startedAt)
  const notify = () => { for (const waiter of [...waiters]) waiter() }
  const describe = (reason, timeoutMs = null) => new Error(`runtime ${reason}: ${JSON.stringify({
    pid: child.pid ?? null, elapsedMs: elapsedMs(), timeoutMs,
    spawnMs: state.spawnMs, readyMs: state.readyMs, exit: state.exit,
    spawnError: state.spawnError, stdinError: state.stdinError,
    stdoutError: state.stdoutError, protocolError: state.protocolError,
    stderr: state.stderr, messages: messages.slice(-8),
  })}`)
  // Wait for stdio to close after an exit so the diagnostic includes stderr.
  const failure = () => state.spawnError || state.protocolError
    || (state.closed && (state.exit || state.stdinError || state.stdoutError || 'closed'))
  let resolveClosed
  const closed = new Promise((resolve) => { resolveClosed = resolve })

  child.on('spawn', () => { state.spawnMs = elapsedMs() })
  child.on('error', (error) => { state.spawnError = error.message; notify() })
  child.on('exit', (code, signal) => {
    state.exit = { code, signal, elapsedMs: elapsedMs() }
    notify()
  })
  child.on('close', () => { state.closed = true; resolveClosed(); notify() })
  child.stdin.on('error', (error) => { state.stdinError = error.message; notify() })
  child.stdout.on('error', (error) => { state.stdoutError = error.message; notify() })
  child.stderr.on('data', (chunk) => {
    state.stderr = (state.stderr + chunk.toString('utf8')).slice(-STDERR_LIMIT)
  })

  output.on('line', (line) => {
    try {
      const message = JSON.parse(line)
      messages.push(message)
      if (message.type === 'ready' && state.readyMs === null) state.readyMs = elapsedMs()
    } catch (error) {
      state.protocolError = error.message
    }
    notify()
  })

  const waitClosed = async (timeoutMs) => {
    let timer
    try {
      return await Promise.race([
        closed.then(() => true),
        new Promise((resolve) => { timer = setTimeout(() => resolve(false), timeoutMs) }),
      ])
    } finally {
      clearTimeout(timer)
    }
  }
  let closing = null

  return {
    child,
    messages,
    get readyElapsedMs() { return state.readyMs },
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
    async waitFor(predicate, timeout = MESSAGE_TIMEOUT_MS) {
      const found = messages.find(predicate)
      if (found) return found
      if (failure()) throw describe('exited before expected message')

      return new Promise((resolve, reject) => {
        const cleanup = () => {
          clearTimeout(timer)
          const index = waiters.indexOf(check)
          if (index >= 0) waiters.splice(index, 1)
        }
        const check = () => {
          const message = messages.find(predicate)
          if (message) { cleanup(); resolve(message) }
          else if (failure()) { cleanup(); reject(describe('exited before expected message')) }
        }
        const timer = setTimeout(() => {
          cleanup()
          reject(describe('timed out waiting for message', timeout))
        }, timeout)
        waiters.push(check)
        check()
      })
    },
    waitForReady(request, type = 'ready') {
      return this.waitFor(
        (message) => message.requestId === request.id && message.type === type,
        STARTUP_READY_TIMEOUT_MS,
      )
    },
    close() {
      if (closing) return closing
      closing = (async () => {
        child.stdin.destroy()
        if (!state.closed) child.kill()
        if (!await waitClosed(1_500)) child.kill('SIGKILL')
        if (!await waitClosed(1_500)) throw describe('did not close after kill')
        output.close()
        child.stdout.destroy()
        child.stderr.destroy()
      })()
      return closing
    },
  }
}

test('cold Node import may take over two seconds without extending business message deadlines', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-runtime-cold-start-'))
  const script = join(directory, 'delayed-runtime.mjs')
  await writeFile(script, `await new Promise((resolve) => setTimeout(resolve, 3_000))\nawait import(${JSON.stringify(pathToFileURL(runtimePath).href)})\n`)
  const runtime = startRuntime(script)
  context.after(async () => {
    await runtime.close()
    await rm(directory, { recursive: true, force: true })
  })

  const initialize = runtime.send('initialize')
  // Retain the old failure mode as a controlled assertion on this same child:
  // the ordinary message window expires while Node is still importing.
  await assert.rejects(runtime.waitFor(
    (message) => message.requestId === initialize.id && message.type === 'ready',
  ), (error) => {
    assert.match(error.message, /"timeoutMs":2000/)
    assert.match(error.message, /"readyMs":null/)
    assert.match(error.message, /"exit":null/)
    return true
  })
  const ready = await runtime.waitForReady(initialize)
  assert.equal(ready.type, 'ready')
  assert.ok(runtime.readyElapsedMs >= 3_000, `ready at ${runtime.readyElapsedMs}ms`)
  console.log(`controlled cold-start readyElapsedMs=${runtime.readyElapsedMs}`)
  const createSession = runtime.send('create_session', {
    conversationId: 'conversation-cold', runtimeSessionId: 'session-cold',
  })
  await runtime.waitFor((message) => message.requestId === createSession.id && message.type === 'session_created')
  await assert.rejects(runtime.waitFor((message) => message.type === 'never-emitted'), (error) => {
    assert.match(error.message, /"timeoutMs":2000/)
    assert.match(error.message, /"readyMs":[0-9]+/)
    return true
  })
})

test('an early Node exit reports its code and bounded stderr before the startup deadline', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-runtime-early-exit-'))
  const script = join(directory, 'exit-runtime.mjs')
  await writeFile(script, `import { writeSync } from 'node:fs'\nwriteSync(2, 'x'.repeat(8_192) + 'controlled startup exit\\n'); process.exit(23)\n`)
  const runtime = startRuntime(script)
  context.after(async () => {
    await runtime.close()
    await rm(directory, { recursive: true, force: true })
  })

  const initialize = runtime.send('initialize')
  await assert.rejects(runtime.waitForReady(initialize), (error) => {
    assert.match(error.message, /exited before expected message/)
    const diagnostic = JSON.parse(error.message.slice(error.message.indexOf('{')))
    assert.equal(diagnostic.exit.code, 23)
    assert.equal(diagnostic.readyMs, null)
    assert.ok(Number.isInteger(diagnostic.spawnMs))
    assert.ok(diagnostic.elapsedMs >= diagnostic.exit.elapsedMs)
    assert.ok(diagnostic.stderr.length <= STDERR_LIMIT)
    assert.ok(diagnostic.stderr.endsWith('controlled startup exit\n'))
    return true
  })
})

test('streams a prompt through the JSONL process boundary', async (context) => {
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize')
  await runtime.waitForReady(initialize)

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
  await runtime.waitForReady(initialize)
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
  let firstRuntime
  let secondRuntime
  context.after(async () => {
    await secondRuntime?.close()
    await firstRuntime?.close()
    await rm(directory, { recursive: true, force: true })
  })

  firstRuntime = startRuntime()
  const initialize = firstRuntime.send('initialize')
  await firstRuntime.waitForReady(initialize)
  const created = firstRuntime.send('create_session', {
    conversationId: 'conversation-resume',
    runtimeSessionId: 'session-resume',
    payload: { sessionPath },
  })
  await firstRuntime.waitFor((message) => message.requestId === created.id && message.type === 'session_created')
  await firstRuntime.close()

  secondRuntime = startRuntime()
  const secondInitialize = secondRuntime.send('initialize')
  await secondRuntime.waitForReady(secondInitialize)
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
  await runtime.waitForReady(initialize)
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
  assert.equal(preflight.payload.toolCallId, 'fake-tool-probe')
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
  const ready = await runtime.waitForReady(initialize)
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
  const rejected = await runtime.waitForReady(invalid, 'request_failed')
  assert.equal(rejected.type, 'request_failed')
  assert.equal(rejected.payload.code, 'runtime.execution_profile.invalid')

  const initialize = runtime.send('initialize', {
    payload: { workLoop: true, executionProfile: 'durable_v2_shadow' },
  })
  const ready = await runtime.waitForReady(initialize)
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
