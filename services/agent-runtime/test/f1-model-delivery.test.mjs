// A-F1 (Node side): the model view of a settled tool result is a projection of
// the durable result, and the projection must never look like it delivered
// content it omitted.
//
// The Host decides whole-file replacement eligibility from the *durable* row
// (see `model_delivery_covered_whole_file_in_tx` in
// `apps/desktop/src-tauri/src/database/repositories/kernel_execution_admission.rs`),
// so what this file pins is the other half of the contract: the engine-side
// projection keeps the same bounds as the Host's, never mutates or re-expands
// the durable payload, and is idempotent — a second projection of an already
// bounded view can never restore the omitted middle.
//
// Constants mirrored from `apps/desktop/src-tauri/src/kernel_compaction.rs`
// (`TOOL_VIEW_HEAD_BYTES` / `TOOL_VIEW_TAIL_BYTES` / `TOOL_VIEW_MAX_BYTES`).
import test from 'node:test'
import assert from 'node:assert/strict'
import { boundToolResultContent, modelToolResultContent, RECEIPT_MARKER } from '../src/tool-view.mjs'

const bytes = (value) => Buffer.byteLength(value, 'utf8')
const REF = 'fox-result://run-f1/read-whole'

// The Host's trusted storage fact for a result it stored completely.
const storedWhole = (text) => ({
  stored: true,
  storedBytes: bytes(text),
  retrievableBytes: bytes(text),
})

const MARKER = 'FOX-F1-MIDDLE-MARKER-7c1f9a2d-NEVER-DELIVERED'
const bigBody = () => 'H'.repeat(15_000) + MARKER + 'T'.repeat(15_000)

test('F1: a whole 30 KB read reaches the provider without its middle, and the durable bytes are untouched', () => {
  const body = bigBody()
  const content = [{ type: 'text', text: body }]
  const projected = boundToolResultContent('read', {
    content,
    resultRef: REF,
    storage: storedWhole(body),
  })

  assert.notEqual(projected, content, 'a 30 KB body must be projected')
  const text = projected[0].text
  assert.equal(text.includes(MARKER), false, 'the omitted middle must not reach the provider')
  assert.ok(bytes(text) < bytes(body), 'the model view must be smaller than the source')
  assert.ok(text.includes(REF), 'the view must name the durable result so the bytes stay reachable')
  // The durable payload is never rewritten, so the Host can still replay the
  // projection from it and reach the same answer.
  assert.equal(content[0].text, body, 'the source result object must be unchanged')
  assert.equal(content[0].text.includes(MARKER), true, 'the durable bytes still carry the marker')
})

test('F1: projecting an already bounded view is a no-op and never restores the omitted middle', () => {
  const body = bigBody()
  const once = boundToolResultContent('read', {
    content: [{ type: 'text', text: body }],
    resultRef: REF,
    storage: storedWhole(body),
  })
  assert.equal(once[0].text.includes(MARKER), false)

  // A second pass — on the live path the frame is projected by the Host and
  // then projected again in the engine, and a replay may project a third time.
  const twice = boundToolResultContent('read', {
    content: once,
    resultRef: REF,
    storage: storedWhole(once[0].text),
  })
  assert.equal(twice, once, 'a bounded view must project to itself')
  assert.equal(twice[0].text.includes(MARKER), false, 'no pass may invent the omitted middle')
})

test('F1: an independent receipt block never protects the body from being bounded', () => {
  const body = bigBody()
  const receipt = `[${RECEIPT_MARKER}] {"executionState":"completed"}`
  const projected = boundToolResultContent('read', {
    content: [
      { type: 'text', text: body },
      { type: 'text', text: receipt },
    ],
    resultRef: REF,
    storage: storedWhole(body),
  })
  assert.equal(projected[1].text, receipt, 'the receipt block is execution evidence and stays verbatim')
  assert.equal(projected[0].text.includes(MARKER), false, 'a separate receipt block must not exempt the body')
  assert.ok(bytes(projected[0].text) < bytes(body))
})

test('F1: the engine bound matches the Host bound on both sides of the limit', () => {
  // Exactly at the Host ceiling: nothing is omitted, so a whole-file read of
  // this size still reaches the model whole.
  const atLimit = 'x'.repeat(9_000)
  const unchanged = boundToolResultContent('read', {
    content: [{ type: 'text', text: atLimit }],
    resultRef: REF,
    storage: storedWhole(atLimit),
  })
  assert.equal(unchanged[0].text, atLimit)

  // One byte over: the projection must omit, and the Host replay would find the
  // same omission.
  const overLimit = 'x'.repeat(9_001)
  const bounded = boundToolResultContent('read', {
    content: [{ type: 'text', text: overLimit }],
    resultRef: REF,
    storage: storedWhole(overLimit),
  })
  assert.notEqual(bounded[0].text, overLimit)
  assert.ok(bytes(bounded[0].text) <= 9_000, `bounded view must fit the budget, got ${bytes(bounded[0].text)}`)
})

test('F1: without a trusted storage fact nothing is omitted, so eligibility is decided by the durable row alone', () => {
  const body = bigBody()
  // No storage fact: the projection keeps every byte and promises nothing, so
  // the model really does receive the whole result.
  const kept = boundToolResultContent('read', {
    content: [{ type: 'text', text: body }],
    resultRef: REF,
  })
  assert.equal(kept[0].text, body, 'an unverified storage fact must not license any omission')
})

test('F1: modelToolResultContent never re-expands a bounded read', () => {
  const body = bigBody()
  const projected = boundToolResultContent('read', {
    content: [{ type: 'text', text: body }],
    resultRef: REF,
    storage: storedWhole(body),
  })
  const view = modelToolResultContent('read', {
    content: projected,
    details: { nextOffset: null, truncated: true },
    resultRef: REF,
    storage: storedWhole(projected[0].text),
  })
  assert.equal(view[0].text, projected[0].text)
  assert.equal(view.some(block => typeof block.text === 'string' && block.text.includes(MARKER)), false)
})
