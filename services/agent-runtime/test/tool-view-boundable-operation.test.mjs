import test from 'node:test'
import assert from 'node:assert/strict'
import { BOUNDABLE_TOOLS_UNDER_TEST, effectiveBoundableTool, modelToolResultContent } from '../src/tool-view.mjs'

const storedWhole = (bytes) => ({ stored: true, storedBytes: bytes, retrievableBytes: bytes })

// The Host dispatches every built-in Office operation through `call_mcp_tool`, so
// judging boundability on the wrapper name made the `office_read` entry in
// `BOUNDABLE_TOOLS` unreachable. A 927-row sheet read then passed through at
// 2.29 MB, past the model window, which detached the live loop.
test('the office wrapper is unwrapped for view bounding', () => {
  for (const inner of ['office_read', 'office_help', 'office_validate']) {
    const input = { serverId: 'fox-office', tool: inner, arguments: {} }
    assert.equal(effectiveBoundableTool('call_mcp_tool', input), inner, inner)
  }
})

test('writes, unknown operations and generic servers keep the wrapper name', () => {
  const write = { serverId: 'fox-office', tool: 'office_create', arguments: {} }
  assert.equal(effectiveBoundableTool('call_mcp_tool', write), 'call_mcp_tool')
  const unknown = { serverId: 'fox-office', tool: 'office_future', arguments: {} }
  assert.equal(effectiveBoundableTool('call_mcp_tool', unknown), 'call_mcp_tool')
  const generic = { serverId: 'some-other-server', tool: 'office_read', arguments: {} }
  assert.equal(effectiveBoundableTool('call_mcp_tool', generic), 'call_mcp_tool')
  // A directly dispatched tool is untouched, and a malformed input cannot widen
  // the whitelist.
  assert.equal(effectiveBoundableTool('read', {}), 'read')
  assert.equal(effectiveBoundableTool('call_mcp_tool', null), 'call_mcp_tool')
  assert.equal(effectiveBoundableTool('call_mcp_tool', 'fox-office'), 'call_mcp_tool')
})

test('an unwrapped office read is actually bounded while a generic mcp call is not', () => {
  const text = 'x'.repeat(40_000)
  const content = [{ type: 'text', text }]
  const storage = storedWhole(Buffer.byteLength(text, 'utf8'))
  const ref = 'fox-result://run-1/call-1'
  const office = modelToolResultContent(
    effectiveBoundableTool('call_mcp_tool', { serverId: 'fox-office', tool: 'office_read' }),
    { content, resultRef: ref, storage },
  )
  assert.ok(office[0].text.length < text.length, 'office_read must be bounded')
  assert.match(office[0].text, /fox-result:\/\/run-1\/call-1/)
  const generic = modelToolResultContent(
    effectiveBoundableTool('call_mcp_tool', { serverId: 'other', tool: 'office_read' }),
    { content, resultRef: ref, storage },
  )
  assert.equal(generic[0].text, text, 'a generic MCP result stays verbatim')
})

test('the exported whitelist still names the office reader', () => {
  // Guards the Rust/Node parity: removing a name from the set must also stop the
  // unwrapping, which is why `effectiveBoundableTool` consults the set.
  assert.ok(BOUNDABLE_TOOLS_UNDER_TEST.has('office_read'))
  assert.equal(BOUNDABLE_TOOLS_UNDER_TEST.has('call_mcp_tool'), false)
})
