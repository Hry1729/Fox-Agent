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
    city: 'Hangzhou',
    temperature: 24,
  })
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
