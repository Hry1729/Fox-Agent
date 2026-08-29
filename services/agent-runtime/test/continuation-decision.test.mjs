import test from 'node:test'
import assert from 'node:assert/strict'
import {
  CONTINUATION_DECISION_SCHEMA,
  CONTINUATION_DECISION_SCHEMA_VERSION,
  CONTINUATION_PROPOSAL_EVENT_TYPE,
  continuationDecisionContract,
  createContinuationDecisionProposal,
  createContinuationDecisionTools,
  normalizeContinuationDecision,
} from '../src/continuation-decision.mjs'
import { resolveExecutionProfile } from '../src/execution-profile.mjs'

test('publishes the shared ContinuationDecision v1 schema fields', () => {
  assert.equal(CONTINUATION_DECISION_SCHEMA_VERSION, 1)
  assert.equal(CONTINUATION_DECISION_SCHEMA.$id, 'fox.runtime.ContinuationDecision.v1')
  assert.deepEqual(CONTINUATION_DECISION_SCHEMA.required, [
    'schemaVersion',
    'decisionId',
    'runId',
    'eventCursor',
    'decision',
    'reasonCode',
    'activeTaskIds',
    'evidenceIds',
    'missingAcceptance',
    'nextAction',
    'retryClass',
    'blockedDependencyRefs',
  ])
  assert.deepEqual(CONTINUATION_DECISION_SCHEMA.properties.reasonCode.enum, [
    'work_remaining',
    'validation_failed',
    'approval_pending',
    'external_dependency_unavailable',
    'budget_exhausted',
    'user_input_required',
    'retry_available',
    'retry_exhausted',
    'acceptance_missing',
    'acceptance_candidate',
    'acceptance_passed',
    'host_audit_required',
    'tool_failed',
    'task_interrupted',
  ])
  assert.deepEqual(CONTINUATION_DECISION_SCHEMA.properties.retryClass.enum, [
    'none',
    'recoverable',
    'retry_limited',
    'non_retryable',
    'host_decides',
  ])
})

test('normalizes bounded proposal data while taking run identity and cursor from Runtime context', () => {
  const normalized = normalizeContinuationDecision({
    runId: 'run-1',
    decision: 'repair',
    reasonCode: 'validation_failed',
    activeTaskIds: [' task-1 ', 'task-1', '', 'task-2'],
    evidenceIds: ['evidence-1'],
    missingAcceptance: ['tests_pass'],
    nextAction: ` run targeted tests ${'x'.repeat(3_000)}`,
    retryClass: 'recoverable',
  }, { runId: 'run-1', eventCursor: 42, proposalIndex: 2 })

  assert.deepEqual(normalized.activeTaskIds, ['task-1', 'task-2'])
  assert.equal(normalized.decisionId, 'decision-run-1-42-2')
  assert.equal(normalized.runId, 'run-1')
  assert.equal(normalized.eventCursor, 42)
  assert.equal(normalized.nextAction.length, 2_000)
  assert.deepEqual(normalized.blockedDependencyRefs, [])
})

test('rejects unknown schema, decision, reason code, cursor, and mismatched run identity', () => {
  const base = { decision: 'continue', reasonCode: 'work_remaining' }
  assert.throws(
    () => normalizeContinuationDecision({ ...base, schemaVersion: 2 }, { runId: 'run-1', eventCursor: 1 }),
    /Unsupported ContinuationDecision schema version/,
  )
  assert.throws(
    () => normalizeContinuationDecision({ ...base, decision: 'cancel' }, { runId: 'run-1', eventCursor: 1 }),
    /Unsupported ContinuationDecision decision/,
  )
  assert.throws(
    () => normalizeContinuationDecision({ ...base, reasonCode: 'looks_done' }, { runId: 'run-1', eventCursor: 1 }),
    /Unsupported ContinuationDecision reasonCode/,
  )
  assert.throws(
    () => normalizeContinuationDecision(base, { runId: 'run-1', eventCursor: -1 }),
    /eventCursor must be a non-negative safe integer/,
  )
  assert.throws(
    () => normalizeContinuationDecision({ ...base, runId: 'run-2' }, { runId: 'run-1', eventCursor: 1 }),
    /does not match the active Runtime run/,
  )
})

test('wraps normalized decisions as proposal-only data and never applies Host state', () => {
  const proposal = createContinuationDecisionProposal({
    decision: 'complete',
    reasonCode: 'acceptance_candidate',
    evidenceIds: ['evidence-1'],
  }, {
    runId: 'run-1',
    eventCursor: 9,
    executionProfileId: 'durable_v2_shadow',
    shadow: true,
  })
  assert.equal(proposal.proposalOnly, true)
  assert.equal(proposal.hostValidationRequired, true)
  assert.equal(proposal.sideEffectsApplied, false)
  assert.equal(proposal.shadow, true)
  assert.equal(proposal.decision, 'complete')
})

test('continuation tool emits one proposal carrier and returns only an acknowledgement', async () => {
  const proposals = []
  const profile = resolveExecutionProfile('durable_v2')
  const tools = createContinuationDecisionTools({
    runId: 'run-7',
    executionProfile: profile,
    getEventCursor: () => 12,
    onProposal: (proposal) => proposals.push(proposal),
  })
  assert.equal(tools.length, 1)
  const result = await tools[0].execute('call-1', {
    decision: 'wait_approval',
    reasonCode: 'approval_pending',
    nextAction: 'wait for Host approval',
  })
  assert.equal(result.hostValidationRequired, true)
  assert.equal(result.transitionApplied, false)
  assert.equal(proposals.length, 1)
  assert.equal(proposals[0].runId, 'run-7')
  assert.equal(proposals[0].eventCursor, 12)
  assert.equal(CONTINUATION_PROPOSAL_EVENT_TYPE, 'run.continuation_proposed')

  assert.deepEqual(
    createContinuationDecisionTools({ executionProfile: resolveExecutionProfile('legacy') }),
    [],
  )
})

test('contract marks Runtime proposals and shadow side effects as non-authoritative', () => {
  const contract = continuationDecisionContract(resolveExecutionProfile('durable_v2_shadow'))
  assert.equal(contract.enabled, true)
  assert.equal(contract.proposalOnly, true)
  assert.equal(contract.hostValidationRequired, true)
  assert.equal(contract.runtimeMayPersistOrApply, false)
  assert.equal(contract.shadowSideEffectsAllowed, false)
  assert.equal(contract.shadow, true)
})
