import test from 'node:test'
import assert from 'node:assert/strict'
import {
  adaptFoxToolsToPi,
  adaptPiToolToFox,
  collectTrustedPiExtensionTools,
  defineFoxTools,
  FOX_TOOL_DEFINITION_VERSION,
} from '../src/tool-adapter.mjs'
import { RESULT_REF_TOOL } from '../src/tool-view.mjs'

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
  // Oversized results from a non-reference tool are published intact: the
  // unified projection declines (execution/fact tools are never reshaped), and
  // there is no structure-breaking character cut and no retrieval promise.
  const oversized = await tools[3].execute('oversized-call', {})
  assert.equal(oversized.details.outputTruncated, false)
  assert.equal(oversized.details.projectionDeclined, true)
  assert.deepEqual(JSON.parse(oversized.content[0].text), { value: 'x'.repeat(120_100) },
    'the oversized payload must stay valid JSON with every byte intact')
  assert.ok(!oversized.content[0].text.includes('[Tool result truncated]'))
  assert.ok(!oversized.content[0].text.includes('可只读取回'))
})

test('oversized results use the unified projection: valid JSON, next, references, receipts', async () => {
  // Sized so the serialized form exceeds the 120k-char pass-through threshold.
  // Retrieval is promised only for results the Host's trusted storage fact says
  // were stored complete — that is what the `storageFor` resolver supplies here,
  // exactly as the production transport must (never from payload size).
  const bigJson = JSON.stringify({
    format: 'docx',
    element: 'paragraph',
    page: 1,
    pageSize: 60,
    properties: Array.from({ length: 370 }, (_, index) => ({ name: `prop-${index}`, summary: 'x'.repeat(300) })),
    next: { page: 2, pageSize: 60 },
  })
  const bigText = `正文 😀 ${'x'.repeat(118_000)}${'资料'.repeat(1_000)}${'😀'.repeat(400)}`
  const storedFacts = new Map()
  const storageFor = (toolCallId, result) => {
    const serialized = typeof result === 'string' ? result : JSON.stringify(result)
    const total = Buffer.byteLength(serialized, 'utf8')
    // The Host confirms it stored this call's complete result.
    storedFacts.set(toolCallId, total)
    return { stored: true, storedBytes: total, retrievableBytes: total }
  }
  const tools = adaptFoxToolsToPi(defineFoxTools([
    piTool({ name: 'office_help', execute: async () => bigJson }),
    piTool({ name: 'read', execute: async () => bigText }),
    piTool({
      name: 'write_file',
      execute: async () => `FOX_EXECUTION_RECEIPT_V1 {"tool":"write_file"} ${'payload '.repeat(30_000)}`,
    }),
  ], {
    source: 'fox-test',
    execution: 'runtime',
    trusted: true,
  }), { runId: 'run-view', storageFor })
  const [officeHelp, read, writeFile] = tools

  // Structured payload: the view stays parseable JSON within the unified
  // budget, shrinks the bulky collection, and keeps navigation reachable.
  const helpResult = await officeHelp.execute('help-call', {})
  assert.ok(bigJson.length > 120_000)
  const view = JSON.parse(helpResult.content[0].text)
  assert.ok(view.foxModelView.bounded)
  assert.equal(view.foxModelView.storageVerified, true)
  assert.equal(view.foxModelView.retrievable, true)
  assert.ok(view.properties.length < 370)
  assert.ok(view.next)
  assert.ok(Buffer.byteLength(helpResult.content[0].text, 'utf8') <= 9_000)

  // Oversized plain text from a reference tool: head/tail bound on code-point
  // boundaries, carrying the stable fox-result:// reference for read-back.
  const readResult = await read.execute('read-call', {})
  assert.ok(readResult.content[0].text.includes('fox-result://run-view/read-call'))
  assert.ok(readResult.content[0].text.includes(RESULT_REF_TOOL))
  assert.ok(!readResult.content[0].text.includes('�'))

  // A receipt-bearing result is execution evidence: never reshaped.
  const receipt = await writeFile.execute('write-call', {})
  assert.ok(receipt.content[0].text.startsWith('FOX_EXECUTION_RECEIPT_V1'))
  assert.equal(receipt.details.outputTruncated, false)
  assert.ok(storedFacts.size >= 2, 'storage facts are resolved per settled call')
})

test('oversized results without a trusted storage fact keep every byte and promise nothing', async () => {
  // The ABC review R5 counterexample: a large JSON payload for a boundable
  // tool must never become unparseable text, and a reference alone must not be
  // presented as proof that Host stored the result. A self-declared storage
  // claim inside the result is ignored too.
  const payload = {
    records: Array.from({ length: 600 }, (_, index) => ({ id: index, body: 'y'.repeat(200) })),
  }
  const tools = adaptFoxToolsToPi(defineFoxTools([
    piTool({
      name: 'office_read',
      execute: async () => ({ ...payload, details: { storage: { stored: true, storedBytes: 999_999 } } }),
    }),
  ], {
    source: 'fox-test',
    execution: 'runtime',
    trusted: true,
  }), { runId: 'run-unverified' })
  const result = await tools[0].execute('unverified-call', {})
  const text = result.content[0].text
  // Whatever shape the projection chose, the payload must stay parseable JSON
  // and must not claim that omitted bytes can be read back.
  const parsed = JSON.parse(text)
  assert.ok(parsed && typeof parsed === 'object', 'the view must remain valid JSON')
  assert.ok(!text.includes('可只读取回'), 'no retrieval promise without a trusted fact')
  if (parsed.foxModelView) {
    assert.equal(parsed.foxModelView.retrievable, false)
    assert.equal(parsed.foxModelView.storageVerified, false)
    assert.equal((parsed.records ?? payload.records).length, 600, 'no record may be dropped')
  } else {
    assert.equal(text, JSON.stringify({ ...payload, details: { storage: { stored: true, storedBytes: 999_999 } } }))
  }
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
