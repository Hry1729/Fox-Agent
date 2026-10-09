import test from 'node:test'
import assert from 'node:assert/strict'
import { createResponseDiagnosticObserver, responseAdmissionTag } from '../src/pi-kernel-response-diagnostic.mjs'

const frame = (tools, finish = null) => `data: ${JSON.stringify({ choices: [{ index: 0, delta: { tool_calls: tools }, finish_reason: finish }] })}\n\n`
async function observe(body) {
  const bytes = new TextEncoder().encode(body)
  const observer = createResponseDiagnosticObserver('openai-completions', async () => new Response(new ReadableStream({
    start(controller) { controller.enqueue(bytes); controller.close() },
  })))
  const response = await observer.fetch('http://local-fixture.invalid', { body: '{"stream":true,"max_tokens":8192}' })
  assert.deepEqual(new Uint8Array(await response.arrayBuffer()), bytes, 'observation forwards every original byte')
  return observer
}
const complete = { role: 'assistant', stopReason: 'toolUse', content: [{ type: 'toolCall', id: 'stable', name: 'read', arguments: {} }] }

test('R1 same-chunk DONE ignores oversized trailing data without changing complete proposals', async () => {
  const observer = await observe(frame([{ index: 0, id: 'stable', function: { arguments: '{}' } }], 'tool_calls')
    + 'data: [DONE]\n\n' + `data: ${'x'.repeat(1_048_600)}\n\n`)
  assert.doesNotThrow(() => observer.assertProposal(complete))
})

test('R1 repeated tool aliases stay bounded while matching the SDK first binding', async () => {
  for (const rotating of ['id', 'index']) {
    let body = frame([{ index: 0, id: 'stable', function: { arguments: '{}' } }])
    for (let i = 1; i <= 10_000; i++) body += frame([{ index: rotating === 'index' ? i : 0,
      id: rotating === 'id' ? `ignored-${i}` : 'stable', function: { arguments: '' } }])
    const observer = await observe(body + frame([], 'tool_calls') + 'data: [DONE]\n\n')
    assert.doesNotThrow(() => observer.assertProposal(complete))
  }
})

test('R1 empty data line flooding exhausts observation budget without claiming protocol invalid', async () => {
  const body = 'data: \n'.repeat(65_537)
    + frame([{ index: 0, id: 'stable', function: { arguments: '{}' } }], 'tool_calls') + 'data: [DONE]\n\n'
  const observer = await observe(body)
  assert.throws(() => observer.assertProposal(complete), error => responseAdmissionTag(error)?.category === 'argument_validation_unavailable')
  const summary = observer.snapshot(null, { turnId: 't', checkpointSeq: 1 })
  assert.equal(summary.providerFinishReason, 'unknown')
})
