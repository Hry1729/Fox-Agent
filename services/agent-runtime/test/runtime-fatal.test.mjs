// A process-level fatal JavaScript error must not be indistinguishable from a
// crashed sidecar.
//
// The real desktop Run 6c5204d6-…-368a9ddb3979 ended with
// `runtime.process_crashed` / "runtime process closed stdout": five runtime
// events, no terminal event and no stderr the Host could show. An uncaught
// exception or an unhandled rejection inside the child produced exactly that
// shape. The runtime now ends the active Run with its normal terminal frame and
// keeps the stack on stderr before it exits non-zero.
import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { mkdtemp, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'
import { createFatalErrorHandler, fatalError, FATAL_CODES } from '../src/runtime-fatal.mjs'

const runtimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))
// `--import` needs a real file:// URL on Windows: a bare `D:/...` path is
// read as the `d:` URL scheme.
const faultInjectorUrl = pathToFileURL(fileURLToPath(new URL('./fixtures/fatal-fault-injection.mjs', import.meta.url))).href

// ---------------------------------------------------------------------------
// The handler itself: injected Run lookup, frame writer, stderr and exit path.
// ---------------------------------------------------------------------------

function recorder() {
  const stderr = []
  const frames = []
  const exits = []
  let scheduled = 0
  return {
    stderr,
    frames,
    exits,
    scheduled: () => scheduled,
    handler: (activeRuns) => createFatalErrorHandler({
      activeRuns,
      reportRunFailure: (run, code, message) => frames.push({ run: run.runId, code, message }),
      stderr: (line) => stderr.push(line),
      exit: (code) => exits.push(code),
      scheduleExit: () => { scheduled += 1 },
    }),
  }
}

test('an unhandled rejection ends every active Run with a terminal code and exits non-zero', () => {
  const r = recorder()
  const handleFatal = r.handler(() => [{ runId: 'run-a' }, { runId: 'run-b' }])
  const result = handleFatal(new Error('detached failure'), 'unhandledRejection')
  assert.deepEqual(result, { code: FATAL_CODES.unhandledRejection, reported: 2, repeated: false })
  assert.deepEqual(r.exits, [1], 'a broken process never reports a clean exit')
  assert.equal(r.scheduled(), 1, 'stdout must be given a chance to flush before exiting')
  assert.deepEqual(r.frames.map(frame => frame.code), [FATAL_CODES.unhandledRejection, FATAL_CODES.unhandledRejection])
  assert.match(r.frames[0].message, /detached failure/)
  assert.match(r.stderr.join(''), /runtime\.unhandled_rejection/)
  assert.match(r.stderr.join(''), /detached failure/)
})

test('an uncaught exception is reported with its own stable code', () => {
  const r = recorder()
  const handleFatal = r.handler(() => [{ runId: 'run-a' }])
  handleFatal(new Error('boom in a timer'), 'uncaughtException')
  assert.equal(r.frames[0].code, FATAL_CODES.uncaughtException)
  assert.equal(r.frames[0].code, 'runtime.uncaught_exception')
  assert.deepEqual(r.exits, [1])
})

test('a second fatal error never emits a second terminal event', () => {
  const r = recorder()
  const handleFatal = r.handler(() => [{ runId: 'run-a' }])
  handleFatal(new Error('first'), 'uncaughtException')
  const second = handleFatal(new Error('second'), 'uncaughtException')
  assert.equal(second.repeated, true)
  assert.equal(second.reported, 0)
  assert.equal(r.frames.length, 1, 'exactly one terminal frame per Run')
  assert.deepEqual(r.exits, [1], 'the exit path runs once')
  assert.equal(r.scheduled(), 1)
})

test('a failure to report one Run never aborts the terminal report or the exit', () => {
  const stderr = []
  const reported = []
  const exits = []
  const handleFatal = createFatalErrorHandler({
    activeRuns: () => [{ runId: 'run-a' }, { runId: 'run-b' }],
    reportRunFailure: (run) => {
      if (run.runId === 'run-a') throw new Error('frame writer is broken')
      reported.push(run.runId)
    },
    stderr: (line) => stderr.push(line),
    exit: (code) => exits.push(code),
    scheduleExit: () => {},
  })
  const result = handleFatal(new Error('root cause'), 'uncaughtException')
  assert.equal(result.reported, 1)
  assert.deepEqual(reported, ['run-b'])
  assert.deepEqual(exits, [1])
  assert.match(stderr.join(''), /terminal Run report failed: frame writer is broken/)
})

test('a failed active-Run lookup still reports to stderr and exits non-zero', () => {
  const stderr = []
  const exits = []
  const handleFatal = createFatalErrorHandler({
    activeRuns: () => { throw new Error('registry unavailable') },
    reportRunFailure: () => { throw new Error('must not be called') },
    stderr: (line) => stderr.push(line),
    exit: (code) => exits.push(code),
    scheduleExit: () => {},
  })
  const result = handleFatal(new Error('root cause'), 'unhandledRejection')
  assert.equal(result.reported, 0)
  assert.deepEqual(exits, [1])
  assert.match(stderr.join(''), /active Run lookup failed: registry unavailable/)
})

test('non-Error rejections are normalized instead of being lost', () => {
  assert.equal(fatalError('plain string').message, 'plain string')
  assert.equal(fatalError({ reason: 'object' }).message, '{"reason":"object"}')
  const circular = {}
  circular.self = circular
  assert.ok(fatalError(circular) instanceof Error, 'an unserializable rejection still yields an Error')
  const r = recorder()
  r.handler(() => [{ runId: 'run-a' }])('plain string', 'unhandledRejection')
  assert.match(r.frames[0].message, /plain string/)
})

// ---------------------------------------------------------------------------
// The real child process: the fault is injected by the test, never by the
// runtime, and it lands inside a live Run.
// ---------------------------------------------------------------------------

function startDelayedProvider(delayMs) {
  const requests = []
  const server = createServer(async (req, res) => {
    let raw = ''
    try {
      for await (const chunk of req) raw += chunk
    } catch {
      // The injected fault kills the child mid-request; a reset body is expected.
      return
    }
    res.on('error', () => {})
    requests.push(raw)
    await new Promise(resolve => setTimeout(resolve, delayMs))
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    for (const choice of [
      { index: 0, delta: { role: 'assistant', content: 'late reply' }, finish_reason: null },
      { index: 0, delta: {}, finish_reason: 'stop' },
    ]) {
      res.write(`data: ${JSON.stringify({ id: 'fatal', object: 'chat.completion.chunk', created: 1, model: 'fatal-test', choices: [choice] })}\n\n`)
    }
    res.end('data: [DONE]\n\n')
  })
  return new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve({
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    requests,
    close: () => { server.closeAllConnections(); server.close() },
  })))
}

function startRuntime(extraEnv = {}) {
  const child = spawn(process.execPath, [runtimePath], {
    stdio: ['pipe', 'pipe', 'pipe'],
    env: { ...process.env, ...extraEnv },
  })
  const closed = new Promise(resolve => child.once('close', (code, signal) => resolve({ code, signal })))
  const frames = []
  const waiters = []
  let stderr = ''
  child.stderr.on('data', (chunk) => { stderr += chunk.toString('utf8') })
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    try { frames.push(JSON.parse(line)) } catch {}
    for (const waiter of [...waiters]) waiter()
  })
  return {
    child,
    frames,
    waitFor(predicate, timeout = 30_000) {
      const existing = frames.find(predicate)
      if (existing) return Promise.resolve(existing)
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`timed out; frames: ${JSON.stringify(frames.slice(-4))}; stderr: ${stderr}`)), timeout)
        const check = () => {
          const message = frames.find(predicate)
          if (!message) return
          clearTimeout(timer)
          resolve(message)
        }
        waiters.push(check)
      })
    },
    waitDeath: () => Promise.race([closed, new Promise(resolve => setTimeout(() => resolve(null), 30_000))]),
    stderr: () => stderr,
    send(type, fields = {}) {
      const request = createEnvelope('request', type, fields)
      child.stdin.write(`${JSON.stringify(request)}\n`)
      return request
    },
    async close() {
      output.close()
      if (child.exitCode === null && child.signalCode === null) child.kill()
      await Promise.race([closed, new Promise(resolve => setTimeout(resolve, 5_000))])
    },
  }
}

async function driveFaultInjectedRun(context, kind) {
  const provider = await startDelayedProvider(5_000)
  const sessions = await mkdtemp(join(tmpdir(), `fox-fatal-${kind}-`))
  const runtime = startRuntime({
    NODE_OPTIONS: `--import=${faultInjectorUrl}`,
    FOX_TEST_FATAL_KIND: kind,
    FOX_TEST_FATAL_DELAY_MS: '75',
  })
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(sessions, { recursive: true, force: true })
  })
  const initialize = runtime.send('initialize', {
    payload: { modelService: {
      apiType: 'openai-completions', modelId: 'fatal-test', baseUrl: provider.baseUrl,
      apiKey: 'local-test', contextWindow: 32768, maxOutputTokens: 1024,
    } },
  })
  assert.equal((await runtime.waitFor(message => message.requestId === initialize.id)).type, 'ready')
  const identity = { conversationId: 'conversation-fatal', runtimeSessionId: 'session-fatal' }
  const create = runtime.send('create_session', {
    ...identity,
    payload: { sessionFormat: 'pi-native', sessionDir: sessions, cwd: sessions },
  })
  assert.equal((await runtime.waitFor(message => message.requestId === create.id)).type, 'session_created')
  runtime.send('prompt', { ...identity, runId: 'run-fatal', payload: { text: 'fatal fault run', projectContext: { projectRoot: null } } })
  const terminal = await runtime.waitFor(message => message.runId === 'run-fatal' && message.payload?.type === 'run.failed', 40_000)
  const death = await runtime.waitDeath()
  return { terminal, death, runtime, provider }
}

test('the real runtime reports an unhandled rejection as a failed Run instead of dying silently', async (context) => {
  const { terminal, death, runtime } = await driveFaultInjectedRun(context, 'rejection')
  assert.equal(terminal.payload.code, 'runtime.unhandled_rejection')
  // The Host stores events under UNIQUE(run_id, seq): the terminal frame must
  // continue the run's own sequence, not restart or duplicate it.
  const events = runtime.frames.filter(message => message.kind === 'event' && message.runId === 'run-fatal')
  const sequences = events.map(message => message.seq)
  assert.deepEqual(sequences, sequences.slice().sort((a, b) => a - b), 'frame sequence is ordered')
  assert.equal(new Set(sequences).size, sequences.length, 'no sequence is reused')
  assert.equal(terminal.seq, Math.max(...sequences.slice(0, -1)) + 1, 'the terminal frame continues the sequence')
  assert.match(terminal.payload.message, /injected fatal fault/)
  assert.equal(runtime.frames.filter(message => message.payload?.type === 'run.failed').length, 1)
  assert.ok(death, 'the process still exits: a fatal error is not swallowed')
  assert.equal(death.code, 1)
  assert.match(runtime.stderr(), /runtime\.unhandled_rejection/)
  assert.match(runtime.stderr(), /injected fatal fault/)
})

test('the real runtime reports an uncaught exception as a failed Run instead of dying silently', async (context) => {
  const { terminal, death, runtime } = await driveFaultInjectedRun(context, 'exception')
  assert.equal(terminal.payload.code, 'runtime.uncaught_exception')
  assert.match(terminal.payload.message, /injected fatal fault/)
  assert.equal(death.code, 1)
  assert.match(runtime.stderr(), /runtime\.uncaught_exception/)
})

test('the fault injector is inert without a configured fault', async (context) => {
  const provider = await startDelayedProvider(0)
  const sessions = await mkdtemp(join(tmpdir(), 'fox-fatal-inert-'))
  const runtime = startRuntime({ NODE_OPTIONS: `--import=${faultInjectorUrl}` })
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(sessions, { recursive: true, force: true })
  })
  const initialize = runtime.send('initialize', {
    payload: { modelService: {
      apiType: 'openai-completions', modelId: 'fatal-test', baseUrl: provider.baseUrl,
      apiKey: 'local-test', contextWindow: 32768, maxOutputTokens: 1024,
    } },
  })
  await runtime.waitFor(message => message.requestId === initialize.id)
  const identity = { conversationId: 'conversation-inert', runtimeSessionId: 'session-inert' }
  const create = runtime.send('create_session', {
    ...identity,
    payload: { sessionFormat: 'pi-native', sessionDir: sessions, cwd: sessions },
  })
  await runtime.waitFor(message => message.requestId === create.id)
  runtime.send('prompt', { ...identity, runId: 'run-inert', payload: { text: 'no fault', projectContext: { projectRoot: null } } })
  const completed = await runtime.waitFor(message => message.runId === 'run-inert' && message.payload?.type === 'run.completed')
  assert.equal(completed.payload.type, 'run.completed')
})
