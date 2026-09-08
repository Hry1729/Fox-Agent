import test from 'node:test'
import assert from 'node:assert/strict'
import {
  adaptFoxToolsToPi,
  adaptPiToolToFox,
  collectTrustedPiExtensionTools,
  defineFoxTools,
  FOX_TOOL_DEFINITION_VERSION,
} from '../src/tool-adapter.mjs'

function piTool(overrides = {}) {
  return {
    name: 'weather_lookup',
    label: 'Weather lookup',
    description: 'Look up the weather for one city.',
    parameters: { type: 'object', properties: { city: { type: 'string' } }, required: ['city'] },
    promptSnippet: 'Look up weather by city.',
    promptGuidelines: ['Use the canonical city name.'],
    prepareArguments: (input) => ({ city: String(input?.city || '').trim() }),
    executionMode: 'parallel',
    renderResult: () => 'Pi TUI only',
    execute: async (_toolCallId, input) => ({ city: input.city, temperature: 24 }),
    ...overrides,
  }
}

test('converts Fox tools to the Pi custom tool contract without leaking adapter metadata', async () => {
  const [foxTool] = defineFoxTools([piTool()], {
    source: 'fox-test',
    execution: 'runtime',
    trusted: true,
  })
  const [adapted] = adaptFoxToolsToPi([foxTool])

  assert.equal(foxTool.fox.schemaVersion, FOX_TOOL_DEFINITION_VERSION)
  assert.equal(foxTool.fox.source, 'fox-test')
  assert.equal(adapted.fox, undefined)
  assert.equal(adapted.promptSnippet, 'Look up weather by city.')
  assert.deepEqual(adapted.promptGuidelines, ['Use the canonical city name.'])
  assert.deepEqual(adapted.prepareArguments({ city: ' Hangzhou ' }), { city: 'Hangzhou' })
  assert.equal(adapted.executionMode, 'parallel')
  assert.equal(adapted.renderResult, undefined)
  assert.deepEqual(await adapted.execute('weather-call', { city: 'Hangzhou' }), {
    content: [{ type: 'text', text: '{"city":"Hangzhou","temperature":24}' }],
    details: {
      structuredResult: { city: 'Hangzhou', temperature: 24 },
      outputTruncated: false,
    },
  })
})

test('preserves Pi-native results while making every concurrent structured result visible to the model', async () => {
  const tools = defineFoxTools([
    piTool({
      name: 'child_agent_list',
      execute: async () => ({ agents: [{ id: 'fox-general' }, { id: 'fox-reviewer' }] }),
    }),
    piTool({
      name: 'read',
      execute: async () => ({
        content: [{ type: 'text', text: 'file contents' }],
        details: { path: 'README.md' },
      }),
    }),
  ], {
    source: 'fox-test',
    execution: 'runtime',
    trusted: true,
  })
  const [childAgentList, read] = adaptFoxToolsToPi(tools)

  const [childAgentResult, readResult] = await Promise.all([
    childAgentList.execute('child-list-call', {}),
    read.execute('read-call', {}),
  ])

  assert.deepEqual(JSON.parse(childAgentResult.content[0].text), {
    agents: [{ id: 'fox-general' }, { id: 'fox-reviewer' }],
  })
  assert.deepEqual(childAgentResult.details, {
    structuredResult: { agents: [{ id: 'fox-general' }, { id: 'fox-reviewer' }] },
    outputTruncated: false,
  })
  assert.deepEqual(readResult, {
    content: [{ type: 'text', text: 'file contents' }],
    details: { path: 'README.md' },
  })
})

test('normalizes primitive, missing, circular, and oversized Fox tool results', async () => {
  const circular = { ok: true }
  circular.self = circular
  const cases = [
    ['string_result', 'plain text', 'plain text'],
    ['missing_result', undefined, 'Tool completed successfully without a return value.'],
    ['circular_result', circular, '{"ok":true,"self":"[Circular]"}'],
    ['oversized_result', { value: 'x'.repeat(120_100) }, null],
  ]
  const tools = adaptFoxToolsToPi(defineFoxTools(cases.map(([name, result]) => piTool({
    name,
    execute: async () => result,
  })), {
    source: 'fox-test',
    execution: 'runtime',
    trusted: true,
  }))

  for (let index = 0; index < tools.length; index += 1) {
    const result = await tools[index].execute(`call-${index}`, {})
    if (cases[index][2] !== null) assert.equal(result.content[0].text, cases[index][2])
    assert.equal(result.content.length, 1)
  }
  assert.deepEqual((await tools[2].execute('circular-details-call', {})).details.structuredResult, {
    ok: true,
    self: '[Circular]',
  })
  const oversized = await tools[3].execute('oversized-call', {})
  assert.equal(oversized.details.outputTruncated, true)
  assert.match(oversized.content[0].text, /\[Tool result truncated\]$/)
})

test('routes imported Pi tools through the Fox Host executor by default', async () => {
  let directExecutions = 0
  const requests = []
  const imported = adaptPiToolToFox(piTool({
    execute: async () => {
      directExecutions += 1
      return { bypassed: true }
    },
  }), {
    source: 'pi-weather',
    hostExecutor: async (request) => {
      requests.push(request)
      return { city: request.input.city, temperature: 21 }
    },
  })

  const result = await imported.execute('weather-call', { city: 'Shanghai' })

  assert.deepEqual(result, { city: 'Shanghai', temperature: 21 })
  assert.equal(directExecutions, 0)
  assert.deepEqual(requests.map(({ source, tool, toolCallId, input }) => ({ source, tool, toolCallId, input })), [{
    source: 'pi-weather',
    tool: 'weather_lookup',
    toolCallId: 'weather-call',
    input: { city: 'Shanghai' },
  }])
})

test('rejects untrusted Pi tools that request direct Runtime execution', () => {
  assert.throws(
    () => adaptPiToolToFox(piTool(), { execution: 'runtime' }),
    (error) => error.code === 'tool_adapter.untrusted_runtime_execution',
  )
})

test('collects tools from a trusted synchronous Pi tool extension', () => {
  const tools = collectTrustedPiExtensionTools((pi) => {
    pi.registerTool(piTool())
  }, {
    source: 'pi-weather-extension',
    execution: 'runtime',
    trusted: true,
  })

  assert.equal(tools.length, 1)
  assert.equal(tools[0].name, 'weather_lookup')
  assert.equal(tools[0].fox.source, 'pi-weather-extension')
})

test('rejects untrusted or lifecycle-dependent Pi extensions', () => {
  assert.throws(
    () => collectTrustedPiExtensionTools(() => {}, { execution: 'runtime' }),
    (error) => error.code === 'tool_adapter.untrusted_extension',
  )
  assert.throws(
    () => collectTrustedPiExtensionTools((pi) => pi.on('before_agent_start', () => {}), {
      source: 'pi-lifecycle-extension',
      execution: 'runtime',
      trusted: true,
    }),
    (error) => error.code === 'tool_adapter.unsupported_extension_api',
  )
})
