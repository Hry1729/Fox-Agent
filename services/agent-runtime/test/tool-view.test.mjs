import test from 'node:test'
import assert from 'node:assert/strict'
import {
  boundToolResultContent,
  boundToolText,
  pagedResume,
  projectStructuredText,
  toolResultRef,
  utf8Prefix,
  utf8Suffix,
} from '../src/tool-view.mjs'

const bytes = (value) => Buffer.byteLength(value, 'utf8')
const replacements = (value) => [...value].filter((char) => char === '\uFFFD').length

function project(tool, text, isError = false, resultRef = null) {
  return boundToolResultContent(tool, { isError, content: [{ type: 'text', text }], resultRef })[0].text
}

test('utf8Prefix/utf8Suffix never split a code point', () => {
  const text = 'a中b😀c'
  // 'a'(1) + '中'(3) = 4 bytes; a 2-byte cut must not include a partial '中'.
  assert.equal(utf8Prefix(text, 2), 'a')
  assert.equal(utf8Prefix(text, 4), 'a中')
  // '😀' is 4 bytes: a 5-byte prefix keeps '中' and must not take half the emoji.
  assert.equal(utf8Prefix(text, 7), 'a中b')
  assert.equal(utf8Prefix(text, 9), 'a中b😀')
  assert.equal(utf8Prefix(text, 11), text)
  assert.equal(replacements(utf8Prefix(text, 5)), 0)
  // Suffix: cutting inside the emoji keeps whole code points only.
  // Byte sizes: 'c'=1, '😀'=4, 'b'=1, '中'=3, 'a'=1.
  assert.equal(utf8Suffix(text, 1), 'c')
  assert.equal(utf8Suffix(text, 4), 'c', 'a 4-byte suffix cannot include the 4-byte emoji plus "c"')
  assert.equal(utf8Suffix(text, 5), '😀c')
  assert.equal(utf8Suffix(text, 6), 'b😀c')
  assert.equal(replacements(utf8Suffix(text, 2)), 0)
  assert.equal(replacements(utf8Suffix(text, 6)), 0)
})

// A reference is what makes an omitted byte reachable, so every fixture that
// expects a bounded view carries one — as every production caller now does.
const REF = 'fox-result://run-1/call-1'

test('bounded multibyte text introduces no replacement character', () => {
  const input = 'a' + '中'.repeat(5_000)
  const output = project('office_help', input, false, REF)
  assert.equal(replacements(output), 0)
  assert.ok(bytes(output) <= 9_000, `bounded view must stay in budget, got ${bytes(output)}`)
})

test('bounded emoji text introduces no replacement character', () => {
  const input = '😀'.repeat(4_000)
  const output = project('read', input, false, REF)
  assert.equal(replacements(output), 0)
  assert.ok(bytes(output) <= 9_000, `bounded view must stay in budget, got ${bytes(output)}`)
})

test('bounded text view still carries a stable result reference', () => {
  const reference = toolResultRef('run-1', 'call-9')
  assert.equal(reference, 'fox-result://run-1/call-9')
  // Non-JSON prose takes the head/tail path and must name the durable result.
  const output = project('read', 'x'.repeat(40_000), false, reference)
  assert.match(output, /fox-result:\/\/run-1\/call-9/)
  assert.match(output, /省略/)
})

test('structured Office catalog stays valid JSON with every property and the next page', () => {
  const catalog = {
    format: 'xlsx', element: 'chart', page: 1, pageSize: 40, totalProperties: 80,
    properties: Array.from({ length: 40 }, (_, index) => ({
      name: `property${index}`, type: 'string', ops: ['set'], hint: '中'.repeat(160),
    })),
    complete: false,
    next: { tool: 'office_help', arguments: { format: 'xlsx', element: 'chart', page: 2, pageSize: 40 } },
  }
  const text = JSON.stringify(catalog)
  assert.ok(bytes(text) < 24 * 1024)

  const output = project('office_help', text)
  assert.ok(bytes(output) <= 9_000, `structured projection must stay in budget, got ${bytes(output)}`)
  const parsed = JSON.parse(output)
  assert.deepEqual(parsed.properties.map((item) => item.name), catalog.properties.map((item) => item.name))
  assert.deepEqual(parsed.next.arguments, catalog.next.arguments)
  assert.equal(parsed.next.tool, 'office_help')
  assert.equal(parsed.foxModelView.bounded, true)
})

test('structured projection keeps navigation verbatim while shrinking bulky leaves', () => {
  const text = JSON.stringify({
    format: 'docx', element: 'paragraph', page: 2,
    properties: [{ name: 'a', hint: 'x'.repeat(4_000) }],
    next: { tool: 'office_help', arguments: { page: 3 }, instruction: 'y'.repeat(1_000) },
  })
  const output = project('office_help', text)
  const parsed = JSON.parse(output)
  assert.equal(parsed.next.arguments.page, 3)
  assert.equal(parsed.next.instruction, 'y'.repeat(1_000))
})

test('structured projection falls back to a valid envelope instead of invalid JSON', () => {
  const text = JSON.stringify(Array.from({ length: 200 }, (_, index) => ({
    name: `row-${index}`, blob: 'z'.repeat(4_000),
  })))
  const output = project('list_mcp_tools', text)
  assert.ok(bytes(output) <= 9_000)
  // Must be parseable: never a mid-JSON character cut.
  const parsed = JSON.parse(output)
  assert.ok(parsed !== null && typeof parsed === 'object')
  assert.equal(parsed.foxModelView.bounded, true)
})

test('structured projection is never applied to small content', () => {
  const text = JSON.stringify({ ok: true, value: 'short' })
  assert.equal(project('office_help', text), text)
})

test('projectStructuredText returns null for non-JSON payloads', () => {
  assert.equal(projectStructuredText('plain prose' , 9_000, null), null)
  assert.equal(projectStructuredText(42, 9_000, null), null)
  assert.equal(projectStructuredText(null, 9_000, null), null)
})

test('projection leaves the supplied source content array untouched', () => {
  const text = JSON.stringify({
    properties: Array.from({ length: 40 }, (_, index) => ({ name: `p${index}`, hint: '中'.repeat(160) })),
  })
  const content = [{ type: 'text', text }]
  const before = JSON.stringify(content)
  boundToolResultContent('office_help', { content })
  assert.equal(JSON.stringify(content), before)
})

test('errors and receipt-bearing results pass through unchanged', () => {
  const text = JSON.stringify({ properties: Array.from({ length: 40 }, (_, index) => ({ name: `p${index}`, hint: '中'.repeat(160) })) })
  assert.equal(project('office_help', text, true), text)
  const receipt = `${text}\nFOX_EXECUTION_RECEIPT_V1 {}`
  assert.equal(project('office_help', receipt), receipt)
})

test('execution and fact tools are never bounded', () => {
  const big = JSON.stringify({ rows: Array.from({ length: 100 }, (_, index) => ({ index, note: 'x'.repeat(200) })) })
  for (const tool of ['write_file', 'attachment_compute', 'run_command', 'child_run_start']) {
    assert.equal(project(tool, big), big)
  }
})

test('generic MCP calls are never bounded because re-read semantics are unknown', () => {
  // A generic call may describe a completed write; "call it again" is not a safe read.
  const writeResult = JSON.stringify({ operation: 'created', id: 'example-only', body: 'x'.repeat(12_000) })
  assert.equal(project('call_mcp_tool', writeResult), writeResult)
  // The tool listing itself is read-only reference material, but it is only
  // bounded while Host keeps the whole of it: omitted entries have to stay
  // reachable, otherwise the view drops tools the model can neither see nor
  // recover.
  const listing = (tools, description) => JSON.stringify({
    tools: Array.from({ length: tools }, (_, index) => ({ name: `mcp_${index}`, description: 'y'.repeat(description) })),
  })
  // Well inside the storage cap: Host keeps everything, so the view may omit and
  // the note must say so.
  const small = listing(60, 200)
  assert.ok(bytes(small) > 9_000, 'fixture must need a projection')
  const projected = project('list_mcp_tools', small)
  assert.notEqual(projected, small)
  assert.equal(JSON.parse(projected).foxModelView.retrievable, true)
  // Past the cap Host keeps only a preview, so dropping entries would lose them
  // everywhere: the payload is published unchanged instead.
  const huge = listing(300, 400)
  assert.equal(project('list_mcp_tools', huge), huge)
})

test('a result Host cannot store whole is never summarized into something unreachable', () => {
  // Head/tail cuts and record drops are only honest while the omitted bytes can
  // be walked back with read_tool_result. Above the storage cap there is no such
  // path, so both the structured projection and the prose fallback must decline.
  const hugeProse = '中'.repeat(60_000)
  assert.ok(bytes(hugeProse) + 4_096 > 128 * 1024)
  assert.equal(boundToolText('read', { text: hugeProse, resultRef: 'fox-result://run/call' }), hugeProse)
  // With storage intact and a reference, the same shape of text is bounded and
  // names the way back.
  const prose = '中'.repeat(4_000)
  const bounded = boundToolText('read', { text: prose, resultRef: 'fox-result://run/call' })
  assert.ok(bytes(bounded) <= 9_000 + 2_048, `bounded view stays near budget, got ${bytes(bounded)}`)
  assert.ok(bounded.includes('read_tool_result'), 'the notice must name the reader that reaches it')
  assert.ok(bounded.includes('fox-result://run/call'))
})

test('toolResultRef refuses incomplete identities', () => {
  assert.equal(toolResultRef('', 'call'), null)
  assert.equal(toolResultRef('run', ''), null)
  assert.equal(toolResultRef(undefined, 'call'), null)
  assert.equal(toolResultRef('run', null), null)
})

test('boundToolText returns null for non-text and small inputs', () => {
  assert.equal(boundToolText('office_help', { text: 'short' }), null)
  assert.equal(boundToolText('office_help', { text: 12345 }), null)
  assert.equal(boundToolText('write_file', { text: 'x'.repeat(50_000) }), null)
})

// --- Paging contract: omitted records must stay reachable -------------------

const propertyName = (index) => `property${String(index).padStart(3, '0')}`

test('a dropped page of a paged catalog resumes at the first omitted property', () => {
  const total = 400
  const catalog = {
    format: 'xlsx', element: 'chart', page: 1, pageSize: 60, totalProperties: total,
    properties: Array.from({ length: 60 }, (_, index) => ({
      name: propertyName(index), type: 'string', ops: ['set'], hint: '中'.repeat(160),
      // Numeric bulk no string-leaf shrink can reduce, so the record-dropping
      // path is actually reached instead of step 1 quietly fitting.
      tagSamples: Array.from({ length: 20 }, () => 123456),
    })),
    complete: false,
    next: {
      tool: 'office_help',
      arguments: { format: 'xlsx', element: 'chart', page: 2, pageSize: 60 },
      instruction: 'page 1/7',
    },
  }
  const text = JSON.stringify(catalog)
  assert.ok(bytes(text) > 9_000, 'fixture must need a projection')

  const output = boundToolText('office_help', { text })
  assert.ok(bytes(output) <= 9_000, `projection must stay in budget, got ${bytes(output)}`)
  const parsed = JSON.parse(output)
  const kept = parsed.properties.map(item => item.name)
  assert.ok(kept.length < 60, 'the fixture must actually drop records')
  assert.equal(parsed.next.tool, 'office_help')

  // Real paging contract of office.rs::shape_help: start = (page-1)*pageSize,
  // pageSize schema 1..=60.
  const resumePage = parsed.next.arguments.page
  const resumePageSize = parsed.next.arguments.pageSize
  assert.ok(Number.isInteger(resumePageSize) && resumePageSize >= 1 && resumePageSize <= 60)
  assert.ok(resumePage >= 1)
  assert.equal(
    (resumePage - 1) * resumePageSize,
    kept.length,
    'the continuation must start exactly at the first omitted record',
  )

  // Walking the real formula from the continuation reaches every omitted record.
  // The continuation's pageSize is kept: switching page sizes mid-walk would
  // create gaps that are an artifact of the walk, not of the continuation.
  const reachable = new Set()
  const pageSize = resumePageSize
  let page = resumePage
  while ((page - 1) * pageSize < total) {
    const start = (page - 1) * pageSize
    for (const name of Array.from({ length: total }, (_, index) => propertyName(index)).slice(start, start + pageSize)) {
      reachable.add(name)
    }
    page += 1
    if (page > 800) break
  }
  for (let index = kept.length; index < total; index += 1) {
    assert.ok(reachable.has(propertyName(index)), `omitted record ${index} must be reachable`)
  }
})

test('an unaddressable payload is returned unchanged instead of losing navigation', () => {
  // A `next` larger than the whole view: nothing can carry it, so nothing is
  // bounded. Cutting here would publish invalid JSON without the continuation.
  const selector = 's'.repeat(12_000)
  const oversizedNext = JSON.stringify({ rows: [{ id: 1 }], next: { tool: 'office_read', arguments: { selector } } })
  assert.ok(bytes(oversizedNext) > 9_000)
  assert.equal(boundToolText('office_read', { text: oversizedNext }), oversizedNext)

  // The third-review probe's paging shape: 220 rows plus a `next` that pages a
  // collection this module has no contract for. Keeping every record is the
  // only honest projection, so the assertion holds by the first disjunct.
  const pagedRows = {
    rows: Array.from({ length: 220 }, (_, index) => ({
      id: index + 1, a: 123456, b: 123456, c: 123456, d: 123456, e: 123456, f: 123456, g: 123456,
    })),
    next: { tool: 'office_read', arguments: { offset: 220, limit: 220 } },
  }
  const text = JSON.stringify(pagedRows)
  assert.ok(bytes(text) > 9_000)
  const output = boundToolText('office_read', { text })
  assert.equal(output, text, 'the payload must be returned unchanged')
  const after = JSON.parse(output)
  const evidence = { inputRows: 220, outputRows: after.rows?.length, nextOffset: after.next?.arguments?.offset }
  assert.ok(
    after.rows?.length === 220 || after.next?.arguments?.offset === after.rows?.length,
    `unreachable via returned next: ${JSON.stringify(evidence)}`,
  )
})

test('pagedResume only emits arguments the office_help schema declares', () => {
  const payload = { format: 'xlsx', element: 'chart', page: 1, pageSize: 60 }
  const resume = pagedResume(payload, 'properties', 30)
  assert.equal(resume.tool, 'office_help')
  assert.deepEqual(Object.keys(resume.arguments).sort(), ['element', 'format', 'page', 'pageSize'])
  assert.equal(resume.arguments.pageSize, 30)
  assert.equal((resume.arguments.page - 1) * resume.arguments.pageSize, 30)
  // The real paging formula must land on the requested index for any keep count.
  for (const kept of [1, 7, 29, 30, 31, 59]) {
    const at = pagedResume(payload, 'properties', kept)
    assert.equal((at.arguments.page - 1) * at.arguments.pageSize, kept)
    assert.ok(at.arguments.pageSize >= 1 && at.arguments.pageSize <= 60)
  }
  // Only the one contract this module can prove, and only with a payload that
  // satisfies the tool's own schema.
  assert.equal(pagedResume(payload, 'rows', 5), null)
  assert.equal(pagedResume({ ...payload, format: '' }, 'properties', 5), null)
  assert.equal(pagedResume({ ...payload, element: undefined }, 'properties', 5), null)
  assert.equal(pagedResume({ ...payload, page: 0 }, 'properties', 5), null)
  assert.equal(pagedResume(payload, 'properties', -5), null)
  assert.equal(pagedResume({ ...payload, pageSize: 0 }, 'properties', 5), null)
  assert.equal(pagedResume({ ...payload, pageSize: 61 }, 'properties', 5), null)
  // Keeping nothing resumes from the first record, which paging still reaches.
  const fromStart = pagedResume(payload, 'properties', 0)
  assert.equal((fromStart.arguments.page - 1) * fromStart.arguments.pageSize, 0)
})

test('a collection with no navigation may drop records, and says how to get them', () => {
  // The reviewer's sample: 220 rows and no `next` were cut to 55 with only an
  // inoperative reference. Dropping is now allowed *because* the reference is a
  // real tool the model can call — and the note has to name it.
  const listing = JSON.stringify({
    records: Array.from({ length: 220 }, (_, index) => ({
      id: index,
      description: 'y'.repeat(60),
    })),
  })
  assert.ok(bytes(listing) > 9_000, 'fixture must need a projection')
  const output = boundToolText('list_mcp_tools', {
    text: listing,
    resultRef: 'fox-result://run-1/call-1',
  })
  assert.ok(bytes(output) <= 9_000, `projection must stay in budget, got ${bytes(output)}`)
  const parsed = JSON.parse(output)
  assert.equal(parsed.foxModelView.bounded, true)
  assert.equal(parsed.foxModelView.retrievable, true)
  assert.equal(parsed.foxModelView.resultRef, 'fox-result://run-1/call-1')
  assert.ok(parsed.foxModelView.omittedItems > 0)
  assert.ok(
    parsed.foxModelView.reRead.includes('read_tool_result'),
    `the re-read hint must name the reader: ${parsed.foxModelView.reRead}`,
  )
  assert.deepEqual(parsed.records.map(item => item.id), Array.from({ length: parsed.records.length }, (_, index) => index))
  // Without a reference there is nothing to call, so nothing may be dropped.
  assert.equal(boundToolText('list_mcp_tools', { text: listing }), listing)
})

test('boundToolResultContent reports unchanged content as unchanged', () => {
  const text = JSON.stringify({
    rows: [{ id: 1 }],
    next: { tool: 'office_read', arguments: { selector: 's'.repeat(12_000) } },
  })
  const content = [{ type: 'text', text }]
  const output = boundToolResultContent('office_read', { content, resultRef: 'fox-result://run/call' })
  assert.equal(output, content, 'an unchanged result must keep its identity')
})
