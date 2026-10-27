import test from 'node:test'
import assert from 'node:assert/strict'
import { createEnvelope } from '../src/protocol.mjs'
import {
  RETIRED_KERNEL_ENGINES,
  SUPPORTED_KERNEL_ENGINE,
  createKernelWorker,
  retiredEngineError,
} from '../src/pi-kernel-worker.mjs'

// Stage one of the single Pi mainline, worker side: the isolated Kernel worker
// executes exactly one model engine. The retired native engines (Codex
// app-server, DeepSeek Harness) keep their records readable, but a request that
// names one is refused before any model request, with a stable code, and is
// never answered by the Pi adapter the caller did not ask for.

const identity = { runId: 'run-retired', conversationId: 'conversation-retired', runtimeSessionId: 'session-retired' }

function startWorker() {
  const out = []
  return { out, worker: createKernelWorker(message => out.push(message)) }
}

function initialize(engineId) {
  return createEnvelope('request', 'kernel.initialize', {
    requestId: 'request-initialize',
    conversationId: identity.conversationId,
    runtimeSessionId: identity.runtimeSessionId,
    runId: identity.runId,
    payload: {
      engineId,
      executionProfileId: 'legacy',
      systemPrompt: 'Use only the supplied durable history.',
      proposalTools: [],
      modelService: { apiType: 'faux', modelId: 'kernel-retirement-test', baseUrl: 'http://localhost', fauxResponses: ['ready'] },
    },
  })
}

test('the only engine this build declares is Pi', () => {
  assert.equal(SUPPORTED_KERNEL_ENGINE, 'pi')
  assert.deepEqual(RETIRED_KERNEL_ENGINES, ['codex', 'deepseek_harness'])
})

test('a retired engine error carries a stable code and names only the engine', () => {
  for (const engine of RETIRED_KERNEL_ENGINES) {
    const error = retiredEngineError(engine)
    assert.equal(error.code, 'kernel_engine_retired')
    assert.match(error.message, /^kernel_engine_retired: /)
    assert.ok(error.message.includes(engine), 'the refusal names the engine')
    assert.doesNotMatch(error.message, /apiKey|baseUrl|http:\/\//)
  }
})

test('a retired engine is refused before any model request and never replaced by Pi', async () => {
  for (const engine of RETIRED_KERNEL_ENGINES) {
    const { out, worker } = startWorker()
    try {
      await worker.handle(initialize(engine))
      const response = out.find(message => message.kind === 'response')
      assert.ok(response, 'the worker answers the initialization')
      assert.equal(response.type, 'request_failed')
      assert.equal(response.payload.code, 'kernel_engine_retired', `${engine} keeps its own code across the wire`)
      assert.ok(response.payload.message.includes(engine))
      assert.equal(response.payload.retryable, false)
      assert.equal(out.filter(message => message.kind === 'event').length, 0, 'nothing was streamed')
      assert.equal(out.some(message => message.type === 'kernel.ready'), false, 'a retired engine is never ready')
    } finally {
      await worker.close().catch(() => {})
    }
  }
})

test('the Pi engine still initializes', async () => {
  const { out, worker } = startWorker()
  try {
    await worker.handle(initialize('pi'))
    assert.ok(out.some(message => message.type === 'kernel.ready'), JSON.stringify(out).slice(0, 400))
  } finally {
    await worker.close().catch(() => {})
  }
})
