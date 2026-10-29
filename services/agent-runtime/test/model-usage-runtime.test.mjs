import test from 'node:test'
import assert from 'node:assert/strict'
import { observeModelUsage } from '../src/model-usage-runtime.mjs'

const model = { id: 'test-model', provider: 'test-provider', api: 'openai-completions', maxTokens: 256, contextWindow: 32768 }
const message = (usage = { input: 7, output: 3, cacheRead: 2, cacheWrite: 1 }) => ({ role: 'assistant',
  content: [{ type: 'text', text: 'done' }], stopReason: 'stop', usage })
const limits = (extra = {}) => ({ timeoutMs: 1000, firstResponseMs: 1000, idleMs: 1000, maxOutputTokens: 128, ...extra })

function streamFor(result, { stall = false, initial = 'text_delta' } = {}) {
  let resolve
  const completion = new Promise(done => { resolve = done })
  return {
    async *[Symbol.asyncIterator]() {
      yield { type: initial, delta: 'working' }
      if (stall) await new Promise(() => {})
      yield { type: 'done', message: result }
      resolve(result)
    },
    result: () => completion,
  }
}

function harness(streamFunction, overrides = {}) {
  const records = [], admissions = [], settlements = []
  const session = { agent: { streamFunction } }
  observeModelUsage(session, {
    runId: 'test-run', stage: 'compaction',
    onRecord: record => records.push(record),
    beforeAttempt: async request => { admissions.push(request); return limits() },
    afterAttempt: async settlement => { settlements.push(settlement) },
    ...overrides,
  })
  return { session, records, admissions, settlements }
}

test('a result-only compaction consumer awaits admission and completion with matching usage identity', async () => {
  let release
  const gate = new Promise(resolve => { release = resolve })
  let sourceStarted = false
  const admissions = []
  const { session, records, settlements } = harness(async (_model, _prompt, options) => {
    sourceStarted = true
    assert.equal(options.maxTokens, 64)
    return streamFor(message())
  }, { beforeAttempt: async request => { admissions.push(request); await gate; return limits({ maxOutputTokens: 64 }) } })
  const opening = session.agent.streamFunction(model, {}, { maxTokens: 200 })
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(sourceStarted, false)
  release()
  const stream = await opening
  assert.equal((await stream.result()).content[0].text, 'done')
  assert.equal(admissions.length, 1)
  assert.equal(admissions[0].stage, 'compaction')
  assert.equal(records[0].stage, 'compaction')
  assert.equal(settlements[0].requestId, admissions[0].requestId)
  assert.equal(records[0].requestId, admissions[0].requestId)
  assert.equal(settlements[0].inputTokens, 10)
  assert.equal(settlements[0].outputTokens, 3)
  assert.equal(settlements[0].status, 'completed')
})

test('each HTTP retry is admitted and settled independently before transport, with its current output cap', async () => {
  const order = [], bodies = [], admissions = [], settlements = [], records = []
  const { session } = harness(async (_model, _prompt, options) => {
    assert.equal(options.maxTokens, 100)
    for (let index = 0; index < 2; index += 1) {
      await options.fetch('http://local-fixture.invalid', { method: 'POST', body: JSON.stringify({ model: model.id, max_tokens: 100 }) })
    }
    return streamFor(message())
  }, {
    onRecord: record => records.push(record),
    beforeAttempt: async request => {
      order.push('admit')
      admissions.push(request)
      return limits({ maxOutputTokens: admissions.length === 1 ? 100 : 25 })
    },
    afterAttempt: async settlement => { order.push('complete'); settlements.push(settlement) },
  })
  const stream = await session.agent.streamFunction(model, {}, { fetch: async (_url, init) => {
    order.push('fetch')
    bodies.push(JSON.parse(init.body))
    return new Response('', { status: bodies.length === 1 ? 503 : 200 })
  } })
  await stream.result()
  assert.deepEqual(order, ['admit', 'fetch', 'complete', 'admit', 'fetch', 'complete'])
  assert.deepEqual(bodies.map(body => body.max_tokens), [100, 25])
  assert.equal(admissions.length, 2)
  assert.notEqual(admissions[0].requestId, admissions[1].requestId)
  assert.deepEqual(records.map(record => record.requestId), admissions.map(request => request.requestId))
  assert.deepEqual(settlements.map(settlement => settlement.requestId), admissions.map(request => request.requestId))
  assert.deepEqual(settlements.map(settlement => settlement.status), ['failed', 'completed'])
  assert.equal(settlements[0].inputTokens, null)
  assert.equal(settlements[0].outputTokens, null)
})

test('a refused model request never reaches the provider', async () => {
  let invoked = false
  const { session, settlements } = harness(() => { invoked = true }, { beforeAttempt: async () => {
    const error = new Error('Host token budget is exhausted.')
    error.code = 'runtime.model_request_denied'
    throw error
  } })
  await assert.rejects(session.agent.streamFunction(model, {}), error => error.code === 'runtime.model_request_denied')
  assert.equal(invoked, false)
  assert.deepEqual(settlements, [])
})

for (const [field, code, initial] of [
  ['timeoutMs', 'runtime.model_request_timeout', 'text_delta'],
  ['firstResponseMs', 'runtime.model_first_response_timeout', 'start'],
  ['idleMs', 'runtime.model_idle_timeout', 'text_delta'],
]) {
  test(`Host ${field} bounds result-only compaction even if the provider ignores abort`, async () => {
    const { session, settlements, records } = harness(() => streamFor(message(), { stall: true, initial }), {
      beforeAttempt: async () => limits({ [field]: 20 }),
    })
    const stream = await session.agent.streamFunction(model, {})
    await assert.rejects(stream.result(), error => error.code === code)
    assert.equal(settlements.length, 1)
    assert.equal(settlements[0].status, 'failed')
    assert.equal(records[0].stage, 'compaction')
  })
}

test('Run cancellation settles only its admitted request and keeps missing usage unknown', async () => {
  const controller = new AbortController()
  const { session, settlements } = harness(() => streamFor(message(), { stall: true }), { signal: controller.signal })
  const stream = await session.agent.streamFunction(model, {})
  controller.abort()
  await assert.rejects(stream.result(), error => error.name === 'AbortError')
  assert.deepEqual(settlements.map(({ inputTokens, outputTokens, status }) => ({ inputTokens, outputTokens, status })),
    [{ inputTokens: null, outputTokens: null, status: 'cancelled' }])
})

test('an all-zero SDK usage object remains unknown at Host completion', async () => {
  const { session, settlements } = harness(() => streamFor(message({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 })))
  const stream = await session.agent.streamFunction(model, {})
  await stream.result()
  assert.equal(settlements[0].inputTokens, null)
  assert.equal(settlements[0].outputTokens, null)
})

test('a failed Host completion is visible to result-only consumers', async () => {
  const { session } = harness(() => streamFor(message()), { afterAttempt: async () => { throw new Error('Host completion could not be persisted.') } })
  const stream = await session.agent.streamFunction(model, {})
  await assert.rejects(stream.result(), /could not be persisted/)
})

test('Legacy observation keeps the caller signal and does not add admission hooks', async () => {
  const controller = new AbortController()
  const records = []
  const session = { agent: { streamFunction: async (_model, _prompt, options) => {
    assert.equal(options.signal, controller.signal)
    return streamFor(message())
  } } }
  observeModelUsage(session, { runId: 'legacy', onRecord: record => records.push(record) })
  const stream = await session.agent.streamFunction(model, {}, { signal: controller.signal })
  for await (const _event of stream) {}
  assert.equal(records.length, 1)
  assert.equal(records[0].stage, 'agent')
})
