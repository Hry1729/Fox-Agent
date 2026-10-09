import assert from 'node:assert/strict'
import test from 'node:test'

import { FOX_RUNTIME_INSTRUCTIONS, runtimeSystemPrompt } from '../src/runtime-instructions.mjs'
import { KERNEL_COMPLETION_CONTRACT, completionRequired } from '../src/kernel-completion.mjs'

/**
 * The completion protocol is opt-in through the *frozen prompt*, so the question
 * "does this request require the final marker?" can only be answered by the prompt the
 * worker was actually handed — not by a config field, and not by a fixture that passes
 * the contract in by hand. These tests walk the real assembly chain and pin what the
 * production composer currently produces, so the answer cannot drift unnoticed.
 */
test('the marker requirement is decided by the frozen prompt, not by a config field', () => {
  // The real production runtime block: this is what a live Run is composed from.
  const composed = runtimeSystemPrompt('You are Fox, a careful general-purpose desktop assistant.')
  // `completionRequired` is the single predicate the worker uses; it must read the
  // composed prompt, so a prompt without the contract cannot require the marker.
  assert.equal(completionRequired(composed), KERNEL_COMPLETION_CONTRACT.length > 0 && composed.includes(KERNEL_COMPLETION_CONTRACT))
  assert.equal(completionRequired(FOX_RUNTIME_INSTRUCTIONS), FOX_RUNTIME_INSTRUCTIONS.includes(KERNEL_COMPLETION_CONTRACT))
  // The predicate itself is exact: only the frozen contract text opts a request in.
  assert.equal(completionRequired(KERNEL_COMPLETION_CONTRACT), true)
  assert.equal(completionRequired(`${KERNEL_COMPLETION_CONTRACT}\nmore`), true)
  // A paraphrase, a truncated copy or the bare marker must not opt in.
  assert.equal(completionRequired('Fox final-answer protocol'), false)
  assert.equal(completionRequired('append <fox-final/> alone on the last line'), false)
  assert.equal(completionRequired(KERNEL_COMPLETION_CONTRACT.slice(0, 64)), false)
  assert.equal(completionRequired(undefined), false)
  assert.equal(completionRequired(''), false)
})

/**
 * Characterization of the production composer as it stands today.
 *
 * This is deliberately an assertion about the *current* behaviour, not an aspiration:
 * no production module imports `KERNEL_COMPLETION_CONTRACT` to place it into a system
 * prompt, so a live Run's frozen prompt does not carry it and `requireCompletion` is
 * false. That makes the marker requirement — and therefore the `missing_final_marker`
 * branch — unreachable in production until the contract is injected into the composer.
 * Enabling it changes real model behaviour, so it is recorded here rather than flipped
 * silently: whoever injects the contract must update this test on purpose.
 */
test('the production composer does not currently opt a live Run into the marker protocol', () => {
  assert.equal(FOX_RUNTIME_INSTRUCTIONS.includes(KERNEL_COMPLETION_CONTRACT), false)
  assert.equal(completionRequired(runtimeSystemPrompt('base prompt')), false)
  // The language contract *is* present, which is why the predicate cannot simply be
  // "the prompt is non-empty" — the two contracts are separate and independently wired.
  assert.equal(FOX_RUNTIME_INSTRUCTIONS.includes('Fox user-facing language contract'), true)
})
