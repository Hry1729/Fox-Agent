import test from 'node:test'
import assert from 'node:assert/strict'
import {
  adaptFoxToolsToPi,
  adaptPiToolToFox,
  collectTrustedPiExtensionTools,
  defineFoxTools,
  FOX_TOOL_DEFINITION_VERSION,
  normalizeFoxToolResultForPi,
} from '../src/tool-adapter.mjs'
import { MAX_MODEL_TOOL_RESULT_BYTES, RESULT_REF_TOOL } from '../src/tool-view.mjs'

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
  // TB-2: an oversized result is capped for the model whatever the tool. With
  // no Host storage fact the view says the middle is gone, and the oversized
  // structured copy is not duplicated into details.
  const oversized = await tools[3].execute('oversized-call', {})
  assert.equal(oversized.details.outputTruncated, true)
  assert.equal(oversized.details.structuredResult, undefined)
  assert.equal(oversized.details.foxResultCap.retrievable, false)
  assert.ok(Buffer.byteLength(oversized.content[0].text, 'utf8') <= MAX_MODEL_TOOL_RESULT_BYTES + 1024)
  assert.ok(oversized.content[0].text.includes('无法取回'))
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

  // A receipt-bearing result keeps its receipt at the head, but obeys the
  // TB-2 ceiling like every other result; its stored copy is retrievable.
  const receipt = await writeFile.execute('write-call', {})
  assert.ok(receipt.content[0].text.startsWith('FOX_EXECUTION_RECEIPT_V1'))
  assert.equal(receipt.details.outputTruncated, true)
  assert.equal(receipt.details.foxResultCap.retrievable, true)
  assert.ok(receipt.content[0].text.includes('fox-result://run-view/write-call'))
  assert.ok(Buffer.byteLength(receipt.content[0].text, 'utf8') <= MAX_MODEL_TOOL_RESULT_BYTES + 2048)
  assert.ok(storedFacts.size >= 2, 'storage facts are resolved per settled call')
})

test('TB-2 oversized results without a trusted storage fact are capped and promise nothing', async () => {
  // A self-declared storage claim inside the result is ignored: only the
  // Host's fact may promise read-back.
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
  const text = result.content.map(block => block.text).join('')
  assert.ok(Buffer.byteLength(text, 'utf8') <= MAX_MODEL_TOOL_RESULT_BYTES + 1024)
  assert.ok(!text.includes('可只读取回'), 'no retrieval promise without a trusted fact')
  if (result.details.outputTruncated) assert.ok(text.includes('无法取回'))
})

for (const mib of [1, 4, 10]) {
  test(`TB-2 a ${mib} MiB Pi-shaped result never exceeds the model ceiling`, () => {
    const text = '数'.repeat(Math.floor((mib * 1024 * 1024) / 3))
    const stored = Buffer.byteLength(text, 'utf8')
    const view = normalizeFoxToolResultForPi('http_request', { content: [{ type: 'text', text }], details: { bytes: stored } }, {
      runId: 'run-mib', toolCallId: `call-${mib}`, storage: { stored: true, storedBytes: stored, retrievableBytes: stored },
    })
    const bytes = view.content.reduce((sum, block) => sum + Buffer.byteLength(block.text ?? '', 'utf8'), 0)
    assert.ok(bytes <= MAX_MODEL_TOOL_RESULT_BYTES + 2048, `${bytes}`)
    assert.ok(!view.content[0].text.includes('\uFFFD'))
    assert.ok(view.content[0].text.includes(`fox-result://run-mib/call-${mib}`))
    assert.ok(view.content[0].text.includes('read_tool_result'))
  })
}

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
