import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createGraphReadonlyTools,
  runReadonlyGraph,
  validateReadonlyGraph,
} from '../src/graph-readonly-tools.mjs'

const graphPolicy = Object.freeze({
  mode: 'read_only_preview',
  maxNodes: 3,
  maxDepth: 1,
  writersAllowed: false,
})

function node(id, dependsOn = []) {
  return { id, task: `Inspect ${id}`, dependsOn }
}

test('validates a bounded depth-one DAG before execution', () => {
  const plan = validateReadonlyGraph({
    nodes: [node('a'), node('b'), node('summary', ['a', 'b'])],
  }, graphPolicy)
  assert.deepEqual(plan.topologicalOrder, ['a', 'b', 'summary'])
  assert.deepEqual(plan.nodes.map(({ id, depth }) => [id, depth]), [
    ['a', 0], ['b', 0], ['summary', 1],
  ])
})

test('rejects graph structure errors before a runner can start', async () => {
  const invalid = [
    { nodes: [] },
    { nodes: [node('a'), node('b'), node('c'), node('d')] },
    { nodes: [node('a'), node('a')] },
    { nodes: [node('a', ['missing'])] },
    { nodes: [node('a', ['a'])] },
    { nodes: [node('a', ['b']), node('b', ['a'])] },
    { nodes: [node('a'), node('b', ['a']), node('c', ['b'])] },
    { nodes: [{ ...node('a'), model: 'override' }] },
  ]
  for (const input of invalid) {
    let calls = 0
    await assert.rejects(
      runReadonlyGraph({
        nodes: input.nodes,
        graphPolicy,
        runNode: async () => { calls += 1; return 'unexpected' },
      }),
    )
    assert.equal(calls, 0)
  }
})

test('rejects a non-canonical graph policy', () => {
  assert.throws(
    () => validateReadonlyGraph({ nodes: [node('a')] }, { ...graphPolicy, maxNodes: 4 }),
    /graph_readonly\.policy_invalid/,
  )
  assert.throws(
    () => validateReadonlyGraph({ nodes: [node('a')] }, { ...graphPolicy, writersAllowed: true }),
    /graph_readonly\.policy_invalid/,
  )
})

test('runs independent roots concurrently and returns input order', async () => {
  let active = 0
  let maximumActive = 0
  const result = await runReadonlyGraph({
    nodes: [node('b'), node('a')],
    graphPolicy,
    runNode: async (current) => {
      active += 1
      maximumActive = Math.max(maximumActive, active)
      await new Promise((resolve) => setTimeout(resolve, 15))
      active -= 1
      return `report-${current.id}`
    },
  })
  assert.equal(maximumActive, 2)
  assert.equal(result.status, 'completed')
  assert.deepEqual(result.nodes.map(({ id }) => id), ['b', 'a'])
  assert.deepEqual(result.acceptedNodeIds, ['b', 'a'])
})

test('starts a dependent node only after every dependency was accepted', async () => {
  const completed = []
  const observedDependencies = []
  const result = await runReadonlyGraph({
    nodes: [node('a'), node('b'), node('summary', ['a', 'b'])],
    graphPolicy,
    runNode: async (current, { dependencyResults }) => {
      if (current.id === 'summary') {
        observedDependencies.push(...dependencyResults.map(({ id }) => id))
        assert.deepEqual(new Set(completed), new Set(['a', 'b']))
      }
      await new Promise((resolve) => setTimeout(resolve, 5))
      completed.push(current.id)
      return `${current.id}-ok`
    },
  })
  assert.equal(result.status, 'completed')
  assert.deepEqual(observedDependencies, ['a', 'b'])
})

test('skips failed descendants while allowing an independent branch to finish', async () => {
  const called = []
  const result = await runReadonlyGraph({
    nodes: [node('bad'), node('independent'), node('dependent', ['bad'])],
    graphPolicy,
    runNode: async (current) => {
      called.push(current.id)
      if (current.id === 'bad') throw new Error('source unavailable')
      return `${current.id}-ok`
    },
  })
  assert.equal(result.status, 'partial')
  assert.deepEqual(called.sort(), ['bad', 'independent'])
  assert.deepEqual(result.nodes.map(({ status }) => status), ['failed', 'accepted', 'skipped'])
  assert.deepEqual(result.acceptedNodeIds, ['independent'])
  assert.equal(result.nodes[0].output, null)
  assert.equal(result.nodes[2].output, null)
})

test('does not accept an empty worker report', async () => {
  const result = await runReadonlyGraph({
    nodes: [node('empty')],
    graphPolicy,
    runNode: async () => '   ',
  })
  assert.equal(result.status, 'partial')
  assert.equal(result.nodes[0].status, 'failed')
  assert.deepEqual(result.acceptedNodeIds, [])
})

test('bounds accepted output and error details', async () => {
  const result = await runReadonlyGraph({
    nodes: [node('large'), node('error')],
    graphPolicy,
    runNode: async (current) => {
      if (current.id === 'error') throw new Error('e'.repeat(2_000))
      return 'x'.repeat(20_000)
    },
  })
  assert.equal(result.nodes[0].output.length, 6_000)
  assert.equal(result.nodes[0].truncated, true)
  assert.equal(result.nodes[1].error.length, 512)
  assert.deepEqual(result.acceptedNodeIds, ['large'])
})

test('propagates cancellation and ignores late success', async () => {
  const controller = new AbortController()
  let started = 0
  const pending = runReadonlyGraph({
    nodes: [node('a'), node('b')],
    graphPolicy,
    signal: controller.signal,
    runNode: async (_current, { signal }) => {
      started += 1
      await new Promise((resolve) => {
        signal.addEventListener('abort', resolve, { once: true })
      })
      return 'late success'
    },
  })
  while (started < 2) await new Promise((resolve) => setTimeout(resolve, 0))
  controller.abort()
  const result = await pending
  assert.equal(result.status, 'cancelled')
  assert.deepEqual(result.nodes.map(({ status }) => status), ['cancelled', 'cancelled'])
  assert.deepEqual(result.acceptedNodeIds, [])
})

test('graph tool fails closed outside graph_readonly_preview', async () => {
  const [tool] = createGraphReadonlyTools({
    profile: { id: 'legacy', graph: { mode: 'disabled', maxNodes: 0, maxDepth: 0, writersAllowed: false } },
    runNode: async () => 'ok',
  })
  await assert.rejects(
    tool.execute('call-1', { nodes: [node('a')] }),
    /graph_readonly\.profile_required/,
  )
})

test('graph tool returns only mechanically accepted reports', async () => {
  const [tool] = createGraphReadonlyTools({
    profile: { id: 'graph_readonly_preview', graph: graphPolicy },
    runNode: async (current) => current.id === 'bad' ? '' : `${current.id}-accepted`,
  })
  const toolResult = await tool.execute('call-1', {
    nodes: [node('good'), node('bad')],
  })
  const parsed = JSON.parse(toolResult.content[0].text)
  assert.equal(parsed.status, 'partial')
  assert.deepEqual(parsed.acceptedNodeIds, ['good'])
  assert.equal(parsed.nodes[1].output, null)
})
