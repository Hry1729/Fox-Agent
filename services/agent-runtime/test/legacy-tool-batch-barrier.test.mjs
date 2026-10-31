// S2 day-1 status-quo regression for the Legacy tool path.
//
// pi-agent-core serializes a batch only when the Agent is configured
// `toolExecution: 'sequential'`, or when some tool in the batch declares
// `executionMode: 'sequential'` (dist/agent-loop.js:374-375); the option
// defaults to 'parallel' (dist/agent.js:147). The Legacy host tools declare nothing, so
// a read and a write in the same batch are dispatched concurrently and the Host
// answers them from a 32-thread pool with no ordering rule.
//
// The first test pins that cause so a future change to either side is caught.
// The second asserts the barrier invariant the single path must satisfy; it is
// `todo` until the Host barrier lands, at which point it should pass unchanged.
import test from 'node:test'
import assert from 'node:assert/strict'
import { Agent } from '@earendil-works/pi-agent-core'
import { createFauxCore, fauxAssistantMessage, fauxToolCall } from '@earendil-works/pi-ai'
import { createHostTools } from '../src/host-tools.mjs'

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function fauxCore() {
  return createFauxCore({
    api: 'faux-legacy-batch',
    provider: 'faux-legacy-batch',
    models: [{ id: 'faux-1', name: 'Faux 1', contextWindow: 128000, maxTokens: 4096 }],
  })
}

test('Legacy host tools declare no executionMode and the SDK defaults to parallel', () => {
  const tools = createHostTools(async () => ({ payload: { result: {} } }))
  assert.ok(tools.length > 0, 'expected the Legacy host tool catalog to be non-empty')

  const declared = tools.filter((tool) => tool.executionMode !== undefined)
  assert.deepEqual(
    declared.map((tool) => `${tool.name}=${tool.executionMode}`),
    [],
    'a Legacy tool declaring executionMode would change which SDK branch a mixed batch takes',
  )

  const core = fauxCore()
  const agent = new Agent({
    initialState: { systemPrompt: 'probe', model: core.getModel(), tools, thinkingLevel: 'off' },
    streamFn: core.streamSimple,
  })
  assert.equal(
    agent.toolExecution,
    'parallel',
    'pi-agent-core defaults toolExecution to parallel, so an undeclared batch runs concurrently',
  )
})

test('a write in the same batch waits for the earlier read to settle', {
  todo: 'S2 barrier: expected to fail until the Host enforces the read-only prefix / write barrier',
}, async () => {
  const catalog = new Map(
    createHostTools(async () => ({ payload: { result: {} } })).map((tool) => [tool.name, tool]),
  )
  for (const name of ['read_attachment', 'write_file']) {
    assert.ok(catalog.has(name), `expected the Legacy catalog to expose ${name}`)
  }

  const windows = []
  const wrap = (name, delayMs) => {
    const source = catalog.get(name)
    // Spread keeps the real definition, including the absence of executionMode.
    return {
      ...source,
      execute: async (toolCallId, params, signal) => {
        const startAt = performance.now()
        await sleep(delayMs)
        try {
          return await source.execute(toolCallId, params, signal)
        } finally {
          windows.push({ name, toolCallId, startAt, endAt: performance.now() })
        }
      },
    }
  }

  const core = fauxCore()
  core.setResponses([
    fauxAssistantMessage(
      [
        fauxToolCall('read_attachment', { attachmentId: 'a1' }, { id: 'call_read_A' }),
        fauxToolCall('write_file', { path: 'B.txt', content: 'x' }, { id: 'call_write_B' }),
      ],
      { stopReason: 'toolUse' },
    ),
    fauxAssistantMessage('done', { stopReason: 'stop' }),
  ])

  const agent = new Agent({
    initialState: {
      systemPrompt: 'probe',
      model: core.getModel(),
      tools: [wrap('read_attachment', 150), wrap('write_file', 0)],
      thinkingLevel: 'off',
    },
    streamFn: core.streamSimple,
  })
  await agent.prompt('go')
  await agent.waitForIdle()

  const read = windows.find((entry) => entry.toolCallId === 'call_read_A')
  const write = windows.find((entry) => entry.toolCallId === 'call_write_B')
  assert.ok(read && write, `both calls must reach execute; got ${JSON.stringify(windows)}`)
  assert.ok(
    write.startAt >= read.endAt,
    'a write must not start before an earlier read in the same batch settles: '
    + `read=[${read.startAt.toFixed(1)},${read.endAt.toFixed(1)}] `
    + `write=[${write.startAt.toFixed(1)},${write.endAt.toFixed(1)}]`,
  )
})
