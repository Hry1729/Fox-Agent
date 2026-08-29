import { Type } from 'typebox'
import { defineFoxTools } from './tool-adapter.mjs'

export const CONTINUATION_DECISION_SCHEMA_VERSION = 1
export const CONTINUATION_PROPOSAL_TOOL_NAME = 'continuation_propose'
export const CONTINUATION_PROPOSAL_EVENT_TYPE = 'run.continuation_proposed'

export const CONTINUATION_DECISIONS = Object.freeze([
  'continue',
  'repair',
  'wait_approval',
  'complete',
  'blocked',
])

export const CONTINUATION_REASON_CODES = Object.freeze([
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

export const CONTINUATION_RETRY_CLASSES = Object.freeze([
  'none',
  'recoverable',
  'retry_limited',
  'non_retryable',
  'host_decides',
])

const CandidateSchema = Type.Object({
  schemaVersion: Type.Optional(Type.Literal(CONTINUATION_DECISION_SCHEMA_VERSION)),
  decisionId: Type.Optional(Type.String({ minLength: 1, maxLength: 200 })),
  decision: Type.Union(CONTINUATION_DECISIONS.map((value) => Type.Literal(value))),
  reasonCode: Type.Union(CONTINUATION_REASON_CODES.map((value) => Type.Literal(value))),
  activeTaskIds: Type.Optional(Type.Array(Type.String(), { maxItems: 100 })),
  evidenceIds: Type.Optional(Type.Array(Type.String(), { maxItems: 100 })),
  missingAcceptance: Type.Optional(Type.Array(Type.String(), { maxItems: 100 })),
  nextAction: Type.Optional(Type.String({ maxLength: 2_000 })),
  retryClass: Type.Optional(Type.Union(CONTINUATION_RETRY_CLASSES.map((value) => Type.Literal(value)))),
  blockedDependencyRefs: Type.Optional(Type.Array(Type.String(), { maxItems: 100 })),
}, { additionalProperties: false })

export const CONTINUATION_DECISION_SCHEMA = Object.freeze({
  $id: 'fox.runtime.ContinuationDecision.v1',
  type: 'object',
  additionalProperties: false,
  required: [
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
  ],
  properties: Object.freeze({
    schemaVersion: Object.freeze({ const: CONTINUATION_DECISION_SCHEMA_VERSION }),
    decisionId: Object.freeze({ type: 'string', minLength: 1, maxLength: 200 }),
    runId: Object.freeze({ type: 'string', minLength: 1, maxLength: 200 }),
    eventCursor: Object.freeze({ type: 'integer', minimum: 0 }),
    decision: Object.freeze({ enum: CONTINUATION_DECISIONS }),
    reasonCode: Object.freeze({ enum: CONTINUATION_REASON_CODES }),
    activeTaskIds: Object.freeze({ type: 'array', items: { type: 'string' }, maxItems: 100 }),
    evidenceIds: Object.freeze({ type: 'array', items: { type: 'string' }, maxItems: 100 }),
    missingAcceptance: Object.freeze({ type: 'array', items: { type: 'string' }, maxItems: 100 }),
    nextAction: Object.freeze({ type: 'string', maxLength: 2_000 }),
    retryClass: Object.freeze({ enum: CONTINUATION_RETRY_CLASSES }),
    blockedDependencyRefs: Object.freeze({ type: 'array', items: { type: 'string' }, maxItems: 100 }),
  }),
})

function boundedString(value, maxLength, fallback = '') {
  const normalized = typeof value === 'string' ? value.trim() : ''
  return (normalized || fallback).slice(0, maxLength)
}

function normalizedIds(value) {
  const ids = []
  const seen = new Set()
  for (const item of Array.isArray(value) ? value : []) {
    const id = boundedString(item, 200)
    if (!id || seen.has(id)) continue
    seen.add(id)
    ids.push(id)
    if (ids.length >= 100) break
  }
  return ids
}

function enumValue(value, allowed, field, fallback) {
  const normalized = boundedString(value, 100, fallback)
  if (!allowed.includes(normalized)) {
    const error = new Error(`Unsupported ContinuationDecision ${field}: ${normalized || '<empty>'}`)
    error.code = 'runtime.continuation.invalid_proposal'
    throw error
  }
  return normalized
}

function normalizedCursor(value) {
  const cursor = Number(value)
  if (!Number.isSafeInteger(cursor) || cursor < 0) {
    const error = new Error('ContinuationDecision eventCursor must be a non-negative safe integer.')
    error.code = 'runtime.continuation.invalid_proposal'
    throw error
  }
  return cursor
}

export function normalizeContinuationDecision(candidate, context = {}) {
  if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) {
    const error = new Error('ContinuationDecision proposal must be an object.')
    error.code = 'runtime.continuation.invalid_proposal'
    throw error
  }
  if (candidate.schemaVersion !== undefined && candidate.schemaVersion !== CONTINUATION_DECISION_SCHEMA_VERSION) {
    const error = new Error(`Unsupported ContinuationDecision schema version: ${candidate.schemaVersion}`)
    error.code = 'runtime.continuation.invalid_proposal'
    throw error
  }
  const runId = boundedString(context.runId ?? candidate.runId, 200)
  if (!runId) {
    const error = new Error('ContinuationDecision runId is required.')
    error.code = 'runtime.continuation.invalid_proposal'
    throw error
  }
  if (candidate.runId !== undefined && boundedString(candidate.runId, 200) !== runId) {
    const error = new Error('ContinuationDecision runId does not match the active Runtime run.')
    error.code = 'runtime.continuation.invalid_proposal'
    throw error
  }
  const eventCursor = normalizedCursor(context.eventCursor ?? candidate.eventCursor ?? 0)
  const decisionId = boundedString(
    candidate.decisionId,
    200,
    `decision-${runId}-${eventCursor}-${Number(context.proposalIndex ?? 1)}`,
  )

  return Object.freeze({
    schemaVersion: CONTINUATION_DECISION_SCHEMA_VERSION,
    decisionId,
    runId,
    eventCursor,
    decision: enumValue(candidate.decision, CONTINUATION_DECISIONS, 'decision'),
    reasonCode: enumValue(candidate.reasonCode, CONTINUATION_REASON_CODES, 'reasonCode'),
    activeTaskIds: normalizedIds(candidate.activeTaskIds),
    evidenceIds: normalizedIds(candidate.evidenceIds),
    missingAcceptance: normalizedIds(candidate.missingAcceptance),
    nextAction: boundedString(candidate.nextAction, 2_000),
    retryClass: enumValue(candidate.retryClass, CONTINUATION_RETRY_CLASSES, 'retryClass', 'host_decides'),
    blockedDependencyRefs: normalizedIds(candidate.blockedDependencyRefs),
  })
}

export function createContinuationDecisionProposal(candidate, context = {}) {
  const decision = normalizeContinuationDecision(candidate, context)
  return Object.freeze({
    ...decision,
    proposalKind: 'runtime_continuation_decision',
    proposalOnly: true,
    hostValidationRequired: true,
    shadow: context.shadow === true,
    sideEffectsApplied: false,
    executionProfileId: boundedString(context.executionProfileId, 100, 'legacy'),
  })
}

export function continuationDecisionContract(profile) {
  const mode = profile?.continuation?.mode ?? 'disabled'
  return Object.freeze({
    schemaVersion: CONTINUATION_DECISION_SCHEMA_VERSION,
    enabled: mode !== 'disabled',
    mode,
    toolName: CONTINUATION_PROPOSAL_TOOL_NAME,
    proposalEventType: CONTINUATION_PROPOSAL_EVENT_TYPE,
    decisions: CONTINUATION_DECISIONS,
    reasonCodes: CONTINUATION_REASON_CODES,
    retryClasses: CONTINUATION_RETRY_CLASSES,
    proposalOnly: true,
    hostValidationRequired: true,
    runtimeMayPersistOrApply: false,
    shadow: profile?.shadow === true,
    shadowSideEffectsAllowed: false,
  })
}

export function createContinuationDecisionTools({
  runId,
  executionProfile,
  getEventCursor = () => 0,
  onProposal = () => {},
} = {}) {
  if (executionProfile?.continuation?.mode === 'disabled') return []
  let proposalIndex = 0
  return defineFoxTools([{
    name: CONTINUATION_PROPOSAL_TOOL_NAME,
    label: 'Propose continuation decision',
    description: 'Propose the current run continuation state for deterministic Fox Host validation. This records a proposal only; it never completes, blocks, repairs, approves, persists, or otherwise changes Host state.',
    parameters: CandidateSchema,
    execute: async (_toolCallId, candidate) => {
      proposalIndex += 1
      const proposal = createContinuationDecisionProposal(candidate, {
        runId,
        eventCursor: getEventCursor(),
        proposalIndex,
        shadow: executionProfile?.shadow === true,
        executionProfileId: executionProfile?.id,
      })
      await onProposal(proposal)
      return {
        proposalRecorded: true,
        decisionId: proposal.decisionId,
        hostValidationRequired: true,
        transitionApplied: false,
        shadow: proposal.shadow,
      }
    },
  }])
}
