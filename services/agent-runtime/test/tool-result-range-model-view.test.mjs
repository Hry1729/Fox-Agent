import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { prepareKernelBatchResume } from '../src/pi-kernel-batch-resume.mjs'
import {
  RESULT_REF_TOOL,
  READ_RESULT_CURSOR_MARKER,
  modelToolResultContent,
  readToolResultNavigationView,
  toolResultRef,
} from '../src/tool-view.mjs'
import { defineFoxTool, adaptFoxToolToPi } from '../src/tool-adapter.mjs'
import { convertMessages } from '../node_modules/@earendil-works/pi-ai/dist/api/openai-completions.js'

const sha256 = (value) => createHash('sha256').update(value, 'utf8').digest('hex')
const model = { id: 'deterministic-fixture', provider: 'openai', api: 'openai-completions', input: ['text'], reasoning: false }

const identity = { runId: 'kernel-run', conversationId: 'kernel-conversation', runtimeSessionId: 'kernel-session', executionProfileId: 'legacy' }

// A fully-persistable payload: Chinese + emoji, big enough to need several pages
// at `limit`, and small enough to stay under the 128 KiB storage cap.
function buildSource() {
  return Array.from({ length: 80 }, (_, i) => `记录${String(i).padStart(2, '0')}:中文内容😀测试样本`).join('\n')
}

// Mirror of the Host `utf8_byte_range` contract: clamp `[offset, offset+limit)`
// onto a code-point boundary, never split a code point, and advance `next`.
function rangeSlice(text, offset, limit) {
  const buf = Buffer.from(text, 'utf8')
  const length = buf.length
  let end = Math.min(length, offset + limit)
  while (end > offset && (buf[end] & 0xc0) === 0x80) end -= 1
  return { end, next: end < length ? end : null, text: buf.subarray(offset, end).toString('utf8') }
}

// What Host's `read_tool_result` execution returns for one range.
function hostRead(source, reference, offset, limit) {
  const { end, next, text } = rangeSlice(source, offset, limit)
  return {
    content: [{ type: 'text', text }],
    details: {
      reference,
      runId: 'source-run',
      toolCallId: 'read-once',
      toolName: 'read',
      status: 'completed',
      offset,
      returnedBytes: end - offset,
      nextOffset: next,
      complete: next === null,
      originalBytes: Buffer.byteLength(source, 'utf8'),
      truncated: false,
      retrievable: true,
      source: 'settled tool result stored by Fox Host',
    },
  }
}

// Extract the cursor block that the model actually received, from the flat
// provider `tool` string (which joins the text blocks with `\n`).
function parseCursor(text) {
  const at = text.lastIndexOf(READ_RESULT_CURSOR_MARKER)
  if (at < 0) return null
  const open = text.indexOf('{', at)
  const close = open < 0 ? -1 : text.indexOf('}', open)
  if (open < 0 || close < 0) return null
  try {
    return JSON.parse(text.slice(open, close + 1))
  } catch {
    return null
  }
}

function controlBinding() {
  const permission = { mode: 'read_only', projectRoot: null, grants: [] }
  return {
    schemaVersion: 1, runId: identity.runId, conversationId: identity.conversationId,
    engineId: 'pi', executionProfileId: identity.executionProfileId, authority: 'authoritative', readOnlyExecutor: 'rust',
    permissionSnapshotId: `sha256:${sha256(JSON.stringify(permission))}`, permission,
    budgets: { modelRequestMs: 120_000, toolExecutionMs: 600_000, runExecutionMs: 1_800_000, approvalWaitMs: 300_000 },
  }
}

test('batch resume: the cursor reaches the final model request and drives a >=3 page reconstruction', () => {
  const source = buildSource()
  const reference = toolResultRef('source-run', 'read-once')
  const limit = 512

  let offset = 0
  let page = 0
  const fragments = []
  let history = [{ role: 'user', content: [{ type: 'text', text: 'read the whole stored result back' }] }]

  while (true) {
    page += 1
    const callId = `range-read-${page}`
    const hostResult = hostRead(source, reference, offset, limit)
    const assistant = {
      role: 'assistant', stopReason: 'toolUse', timestamp: page + 1,
      content: [{ type: 'toolCall', id: callId, name: RESULT_REF_TOOL, arguments: { reference, offset, limit } }],
    }
    const frame = {
      schemaVersion: 1, turnId: 'turn-1', batchId: `batch-${page}`,
      idempotencyKey: `tool-batch-delivery:batch-${page}`, checkpointSeq: 8 + page,
      history,
      assistantMessage: assistant,
      tools: [{ toolCallId: callId, tool: RESULT_REF_TOOL, sourceOrder: 0, canonicalInput: { reference, offset, limit }, state: 'completed', result: hostResult }],
    }
    const prepared = prepareKernelBatchResume({ ...identity, payload: { controlBinding: controlBinding(), batchResume: frame } }, identity)
    const wire = convertMessages(model, { messages: prepared.messages }, {})
    const toolMsg = wire.filter((message) => message.role === 'tool').at(-1)
    const nav = parseCursor(toolMsg.content)
    assert.ok(nav, `page ${page}: cursor block must reach the model request`)
    assert.equal(nav.reference, reference)
    assert.equal(nav.offset, offset)
    assert.equal(nav.retrievable, true)
    assert.equal(nav.truncated, false)
    assert.equal(nav.nextOffset, hostResult.details.nextOffset)

    // Accumulate the model-visible history so the next page replays prior reads.
    history = [...history, assistant, prepared.messages.at(-1)]
    fragments.push(Buffer.from(toolMsg.content).subarray(0, nav.returnedBytes).toString('utf8'))

    if (nav.complete) break
    assert.ok(nav.nextOffset > offset, `page ${page}: cursor must advance`)
    offset = nav.nextOffset
  }

  assert.ok(page >= 3, `expected at least 3 pages, got ${page}`)
  assert.equal(sha256(fragments.join('')), sha256(source), 'reconstructed hash must equal the source')
  assert.equal(fragments.join(''), source)
})

test('live settled projection: the fragment stays first and the cursor reaches the provider request', () => {
  const source = buildSource()
  const reference = toolResultRef('source-run', 'read-once')
  const hostResult = hostRead(source, reference, 0, 512)

  const content = modelToolResultContent(RESULT_REF_TOOL, {
    isError: false,
    content: hostResult.content,
    details: hostResult.details,
    resultRef: toolResultRef('reading-run', 'range-read'),
  })

  assert.equal(content.length, 2)
  assert.equal(content[0].text, hostResult.content[0].text, 'the raw fragment stays the first block')
  assert.ok(content[1].text.startsWith(READ_RESULT_CURSOR_MARKER))
  assert.equal(content[1].text.search('\n'), -1, 'the cursor block is a single line for clean extraction')

  const messages = [{ role: 'toolResult', toolCallId: 'range-read', toolName: RESULT_REF_TOOL, content, isError: false }]
  const wire = convertMessages(model, { messages }, {})
  const nav = parseCursor(wire.find((message) => message.role === 'tool').content)
  assert.ok(nav)
  assert.equal(nav.complete, false, 'a 512-byte first page of a ~3 KiB source is not complete')
  assert.ok(nav.nextOffset > 0)
})

test('legacy path: adaptFoxToolToPi surfaces the cursor into the model-visible content', async () => {
  const source = buildSource()
  const reference = toolResultRef('source-run', 'read-once')
  const hostResult = hostRead(source, reference, 0, 512)

  const foxTool = defineFoxTool({
    name: RESULT_REF_TOOL,
    label: 'Read stored tool result',
    description: 'read a range of a stored result',
    parameters: { type: 'object' },
    execute: () => hostResult,
  }, { source: 'fox-host', execution: 'host', trusted: true })

  const result = await adaptFoxToolToPi(foxTool).execute('range-read', { reference, offset: 0, limit: 512 })
  assert.equal(result.content.length, 2)
  assert.equal(result.content[0].text, hostResult.content[0].text)
  assert.ok(result.content[1].text.startsWith(READ_RESULT_CURSOR_MARKER))

  const messages = [{ role: 'toolResult', toolCallId: 'range-read', toolName: RESULT_REF_TOOL, content: result.content, isError: false }]
  const wire = convertMessages(model, { messages }, {})
  const nav = parseCursor(wire.find((message) => message.role === 'tool').content)
  assert.ok(nav)
  assert.equal(nav.nextOffset, hostResult.details.nextOffset)
})

test('a non-retrievable stored preview is distinguishable in the model view', () => {
  const result = {
    content: [{ type: 'text', text: 'the bounded preview that Host kept' }],
    details: {
      reference: toolResultRef('source-run', 'read-once'),
      offset: 0,
      returnedBytes: 32,
      nextOffset: null,
      complete: true,
      originalBytes: 200_000,
      truncated: true,
      retrievable: false,
    },
  }
  const content = modelToolResultContent(RESULT_REF_TOOL, { content: result.content, details: result.details })
  assert.equal(content.length, 2)
  const wire = convertMessages(model, { messages: [{ role: 'toolResult', toolCallId: 'preview-read',
    toolName: RESULT_REF_TOOL, content, isError: false }] }, {})
  const nav = parseCursor(wire[0].content)
  assert.ok(nav)
  assert.equal(nav.complete, true)
  assert.equal(nav.retrievable, false)
  assert.equal(nav.truncated, true)
  assert.equal(nav.nextOffset, null)
})

test('non-read_tool_result tools never leak a cursor block', () => {
  const content = modelToolResultContent('read', { content: [{ type: 'text', text: 'plain text' }], details: { nextOffset: 5 } })
  assert.equal(content.length, 1, 'whitelist applies only to read_tool_result')
  assert.equal(readToolResultNavigationView({ kept: 'private', nextOffset: 3 }).nextOffset, 3, 'unlisted keys are dropped')
  assert.ok(!('kept' in readToolResultNavigationView({ kept: 'private', nextOffset: 3 })))
})

test('replayed views keep a single cursor across Rust key order and repeated Legacy adaptation', async () => {
  const result = hostRead(buildSource(), toolResultRef('source-run', 'read-once'), 0, 512)
  const before = JSON.stringify(result)
  const once = modelToolResultContent(RESULT_REF_TOOL, result)
  const nav = readToolResultNavigationView(result.details)
  // Rust serde_json sorts keys, unlike the JS whitelist insertion order.
  once[1] = { type: 'text', text: `${READ_RESULT_CURSOR_MARKER} ${JSON.stringify(
    Object.fromEntries(Object.entries(nav).sort(([a], [b]) => a.localeCompare(b))))}` }
  assert.deepEqual(modelToolResultContent(RESULT_REF_TOOL, { ...result, content: once }), once)
  const foxTool = defineFoxTool({ name: RESULT_REF_TOOL, label: 'Range', description: 'Read stored bytes',
    parameters: { type: 'object' }, execute: () => ({ ...result, content: once }) })
  const adapted = await adaptFoxToolToPi(foxTool).execute('range', {})
  assert.deepEqual(adapted.content, once)
  assert.equal(JSON.stringify(result), before)
  // A raw fragment identical to a cursor is still data and must stay intact.
  const fragment = [{ type: 'text', text: once[1].text }]
  const projected = modelToolResultContent(RESULT_REF_TOOL, { ...result, content: fragment })
  assert.equal(projected.length, 2)
  assert.deepEqual(projected[0], fragment[0])
  assert.deepEqual(modelToolResultContent(RESULT_REF_TOOL, { ...result, isError: true }), result.content)
})
