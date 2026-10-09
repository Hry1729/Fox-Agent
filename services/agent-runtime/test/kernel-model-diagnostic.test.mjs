import assert from 'node:assert/strict'
import test from 'node:test'

import { incompleteResponseReason, modelFailureDiagnostic } from '../src/kernel-completion.mjs'

/**
 * The diagnostic names the internal branch and reports only observed facts: a round
 * whose message was never available contributes no counters, so the Host reads them
 * as unobserved rather than as zero.
 */
test('the incomplete-response reason names the branch that fired', () => {
  const round = (text, stopReason = 'stop', extra = []) => ({
    role: 'assistant', stopReason,
    content: [{ type: 'thinking', thinking: 'reasoning' }, { type: 'text', text }, ...extra],
  })
  assert.equal(incompleteResponseReason(round('')), 'empty_final_answer')
  assert.equal(incompleteResponseReason(round('   ')), 'empty_final_answer')
  assert.equal(incompleteResponseReason(round('an answer with no marker'), true), 'missing_final_marker')
  // Not completion-required: the same text is a finished answer, and the stop reason
  // decides whether the provider's own limit ended it.
  assert.equal(incompleteResponseReason(round('an answer with no marker'), false), 'transport_no_output')
  assert.equal(incompleteResponseReason(round('truncated...', 'length'), false), 'output_length_limit')
  assert.equal(incompleteResponseReason(round('done\n<fox-final/>'), true), 'transport_no_output')
  // No settled message at all: the only honest reason.
  assert.equal(incompleteResponseReason(null, null), 'transport_no_output')
})

test('the diagnostic carries observations and omits what was never seen', () => {
  const observed = modelFailureDiagnostic({
    message: {
      role: 'assistant', stopReason: 'stop',
      content: [
        { type: 'thinking', thinking: '思考两字' },
        { type: 'text', text: 'partial answer' },
        { type: 'toolCall', toolCallId: 'a', toolName: 'read', args: {} },
      ],
    },
    reason: 'missing_final_marker',
    completionRequired: true,
    dispatchId: 'model-batch:run:30',
    effectKey: 'deliver-batch:model-batch:run:30',
  })
  assert.deepEqual(observed, {
    reason: 'missing_final_marker',
    stopReason: 'stop',
    textChars: 14,
    thinkingChars: 4,
    toolCallCount: 1,
    markerSeen: false,
    completionRequired: true,
    dispatchId: 'model-batch:run:30',
    effectKey: 'deliver-batch:model-batch:run:30',
  })

  // No message: counters are absent rather than zero, and no false is invented.
  const withoutMessage = modelFailureDiagnostic({ reason: 'transport_no_output' })
  assert.deepEqual(withoutMessage, { reason: 'transport_no_output' })
  assert.equal('textChars' in withoutMessage, false)
  assert.equal('markerSeen' in withoutMessage, false)
  assert.equal('completionRequired' in withoutMessage, false)

  // Nothing observed at all is null, not an object full of fabricated zeros.
  assert.equal(modelFailureDiagnostic({}), null)
  assert.equal(modelFailureDiagnostic(), null)
})

/**
 * The six causes behind a settled failure must be told apart, each driven through the
 * real decision functions. A fact the exit could not observe stays absent; a fact it
 * did observe survives even when it is a real `false` or `0`.
 */
test('each failure shape is named, and an unobserved fact is never invented', () => {
  const round = (text, stopReason = 'stop') => ({
    role: 'assistant', stopReason, content: [{ type: 'text', text }],
  })

  // 1. Empty body: the round settled but produced no public text at all.
  assert.equal(incompleteResponseReason(round(''), true), 'empty_final_answer')

  // 2. Missing completion marker, and this request really did require it.
  assert.equal(incompleteResponseReason(round('an answer'), true), 'missing_final_marker')
  const required = modelFailureDiagnostic({
    message: round('an answer'), reason: incompleteResponseReason(round('an answer'), true),
    completionRequired: true,
  })
  assert.equal(required.reason, 'missing_final_marker')
  assert.equal(required.completionRequired, true)
  assert.equal(required.markerSeen, false)

  // 3. The same text when the marker was not required is not "missing" anything.
  assert.equal(incompleteResponseReason(round('an answer'), false), 'transport_no_output')
  const notRequired = modelFailureDiagnostic({
    message: round('an answer'), reason: incompleteResponseReason(round('an answer'), false),
    completionRequired: false,
  })
  assert.equal(notRequired.reason, 'transport_no_output')
  // A real, observed `false` is carried; only an unobserved one would be absent.
  assert.equal(notRequired.completionRequired, false)

  // 4. Output-length limit: the provider cut the round off.
  assert.equal(incompleteResponseReason(round('truncated', 'length'), true), 'output_length_limit')
  const limited = modelFailureDiagnostic({
    message: round('truncated', 'length'), reason: 'output_length_limit',
    completionRequired: true,
  })
  assert.equal(limited.reason, 'output_length_limit')
  assert.equal(limited.stopReason, 'length')

  // 5. Stream interrupted / transport failure with no settled message: only the branch
  //    is known, so nothing else may be asserted — this is the "zero output" trap.
  const transport = modelFailureDiagnostic({ reason: 'transport_no_output' })
  assert.deepEqual(transport, { reason: 'transport_no_output' })
  for (const absent of ['textChars', 'thinkingChars', 'toolCallCount', 'markerSeen', 'stopReason']) {
    assert.equal(absent in transport, false, `${absent} was never observed and must stay absent`)
  }
  // A round that settled with no measurable content is a *real* zero, and is kept.
  const settledEmpty = modelFailureDiagnostic({ message: round(''), reason: 'empty_final_answer' })
  assert.equal(settledEmpty.textChars, 0)
  assert.equal(settledEmpty.toolCallCount, 0)
  assert.equal(settledEmpty.markerSeen, false)

  // 6. Nothing observed at all: the caller must not invent an object.
  assert.equal(modelFailureDiagnostic({}), null)
})

/**
 * "Marker seen" and "marker missing" have to be decided by the same predicate, or a
 * marker quoted mid-answer would be reported as seen while the branch fired as missing.
 */
test('markerSeen and the missing-marker branch agree on the same predicate', () => {
  const round = text => ({ role: 'assistant', stopReason: 'stop', content: [{ type: 'text', text }] })
  const quoted = round('the literal <fox-final/> is inline, not a signal')
  assert.equal(incompleteResponseReason(quoted, true), 'missing_final_marker')
  assert.equal(modelFailureDiagnostic({ message: quoted }).markerSeen, false)
  const closed = round('an answer\n<fox-final/>')
  assert.equal(modelFailureDiagnostic({ message: closed }).markerSeen, true)
  assert.notEqual(incompleteResponseReason(closed, true), 'missing_final_marker')
})
