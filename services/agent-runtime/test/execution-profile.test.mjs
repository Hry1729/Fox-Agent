import test from 'node:test'
import assert from 'node:assert/strict'
import { Check } from 'typebox/value'
import {
  applyExecutionProfileToTools,
  executionProfilePrompt,
  executionProfileSnapshot,
  GRAPH_REVIEWER_DECISION_OUTPUT_SCHEMA,
  resolveExecutionProfile,
  supportedExecutionProfileIds,
} from '../src/execution-profile.mjs'

test('resolves the five supported execution profiles into versioned stable snapshots', () => {
  assert.deepEqual(supportedExecutionProfileIds(), [
    'legacy',
    'durable_v2_shadow',
    'durable_v2',
    'graph_reviewer_v1',
    'graph_readonly_preview',
  ])

  const legacy = executionProfileSnapshot(resolveExecutionProfile('legacy'))
  const shadow = executionProfileSnapshot(resolveExecutionProfile('durable_v2_shadow'))
  const durable = executionProfileSnapshot(resolveExecutionProfile('durable_v2'))
  const reviewer = executionProfileSnapshot(resolveExecutionProfile('graph_reviewer_v1'))
  const graph = executionProfileSnapshot(resolveExecutionProfile('graph_readonly_preview'))

  assert.equal(legacy.schemaVersion, 1)
  assert.equal(legacy.strategies.completionAudit, 'legacy')
  assert.equal(legacy.continuation.mode, 'disabled')
  assert.equal(legacy.sideEffectsAllowed, true)
  assert.equal(shadow.continuation.mode, 'shadow')
  assert.equal(shadow.shadow, true)
  assert.equal(shadow.sideEffectsAllowed, false)
  assert.equal(durable.continuation.mode, 'propose')
  assert.equal(durable.continuation.hostValidationRequired, true)
  assert.deepEqual(reviewer.strategies, {
    completionAudit: 'strict_v2',
    validationPolicy: 'high_risk_v1',
    promptPolicy: 'graph_reviewer_v1',
  })
  assert.equal(reviewer.continuation.mode, 'disabled')
  assert.deepEqual(reviewer.graph, { mode: 'disabled', maxNodes: 0, maxDepth: 0, writersAllowed: false })
  assert.equal(reviewer.toolPolicy, 'graph_reviewer_read_only')
  assert.equal(reviewer.sideEffectsAllowed, false)
  assert.deepEqual(graph.graph, {
    mode: 'read_only_preview',
    maxNodes: 3,
    maxDepth: 1,
    writersAllowed: false,
  })
  assert.deepEqual(
    [legacy.hash, shadow.hash, durable.hash, reviewer.hash, graph.hash],
    ['7b78877c1383f054', 'c366ff34ec65419c', 'a6f44bb7c868e74e', 'd2f059f5dbad815e', 'd8abc7f9f3e05a80'],
  )
  assert.equal(graph.hash, executionProfileSnapshot(resolveExecutionProfile('graph_readonly_preview')).hash)
})

test('gives the hidden Graph Reviewer only the exact local read-only proof tools', () => {
  const reviewer = resolveExecutionProfile('graph_reviewer_v1')
  const tools = [
    { name: 'read' },
    { name: 'ls' },
    { name: 'find' },
    { name: 'grep' },
    { name: 'read_attachment' },
    { name: 'sqlite_read' },
    { name: 'structured_data' },
    { name: 'work_snapshot_get' },
    { name: 'continuation_propose' },
    { name: 'graph_readonly_run' },
    { name: 'graph_readonly_snapshot_get' },
    { name: 'graph_readonly_node_review' },
    { name: 'acceptance_submit' },
    { name: 'task_attempt_finish' },
    { name: 'child_run_start' },
    { name: 'approval_request' },
    { name: 'write_file' },
    { name: 'run_command' },
    { name: 'memory_search' },
    { name: 'search_knowledge' },
    { name: 'list_mcp_tools' },
  ]
  const selected = applyExecutionProfileToTools(tools, reviewer)
  assert.deepEqual(selected.tools.map(({ name }) => name), ['read', 'ls', 'find', 'grep'])
  assert.deepEqual(
    selected.excludedTools.map(({ name }) => name),
    tools.slice(4).map(({ name }) => name),
  )
  assert.ok(selected.excludedTools.every(({ profile }) => profile === 'graph_reviewer_v1'))
})

test('freezes the Graph Reviewer exact four-key structured finding output schema', () => {
  const schema = GRAPH_REVIEWER_DECISION_OUTPUT_SCHEMA
  assert.equal(schema.additionalProperties, false)
  assert.deepEqual(schema.required, ['criteria', 'recommendation', 'summary', 'findings'])
  assert.deepEqual(Object.keys(schema.properties), ['criteria', 'recommendation', 'summary', 'findings'])
  assert.deepEqual(schema.properties.recommendation.enum, ['pass', 'revise', 'inconclusive'])
  assert.equal(schema.properties.criteria.minItems, 0)
  assert.equal(schema.properties.criteria.maxItems, 8)
  assert.deepEqual(schema.properties.criteria.items.required, ['criterion', 'status', 'toolCallIds'])
  assert.equal(schema.properties.criteria.items.additionalProperties, false)
  assert.deepEqual(schema.properties.criteria.items.properties.status.enum, ['passed', 'failed', 'inconclusive'])
  assert.equal(schema.properties.criteria.items.properties.criterion.maxLength, 1_000)
  assert.equal(schema.properties.criteria.items.properties.toolCallIds.minItems, 1)
  assert.equal(schema.properties.criteria.items.properties.toolCallIds.maxItems, 16)
  assert.equal(schema.properties.criteria.items.properties.toolCallIds.uniqueItems, true)
  assert.equal(schema.properties.summary.maxLength, 4_000)
  assert.equal(schema.properties.findings.minItems, 0)
  assert.equal(schema.properties.findings.maxItems, 16)
  assert.deepEqual(schema.properties.findings.items.required, ['criterion', 'severity', 'title', 'detail', 'toolCallIds'])
  assert.equal(schema.properties.findings.items.additionalProperties, false)
  assert.deepEqual(
    schema.properties.findings.items.properties.severity.enum,
    ['critical', 'high', 'medium', 'low', 'info'],
  )
  assert.equal(schema.properties.findings.items.properties.title.maxLength, 200)
  assert.equal(schema.properties.findings.items.properties.detail.maxLength, 2_000)
  assert.equal(schema.properties.findings.items.properties.toolCallIds.minItems, 1)
  assert.equal(schema.properties.findings.items.properties.toolCallIds.maxItems, 16)
  assert.equal(schema.properties.findings.items.properties.toolCallIds.uniqueItems, true)
  assert.equal(Object.isFrozen(schema), true)
  assert.equal(Object.isFrozen(schema.properties.findings.items), true)
  const revise = {
    criteria: [{ criterion: 'No unsafe write occurs.', status: 'failed', toolCallIds: ['review-tool-1'] }],
    recommendation: 'revise',
    summary: 'The proof shows an unsafe write path.',
    findings: [{
      criterion: 'No unsafe write occurs.',
      severity: 'high',
      title: 'Unsafe write path remains',
      detail: 'The inspected call path still reaches a write operation.',
      toolCallIds: ['review-tool-1'],
    }],
  }
  assert.equal(Check(schema, revise), true)
  assert.equal(Check(schema, { ...revise, extra: true }), false)
  assert.equal(Check(schema, { ...revise, findings: [{ ...revise.findings[0], severity: 'blocker' }] }), false)
  assert.equal(Check(schema, { ...revise, findings: [{ ...revise.findings[0], title: '' }] }), false)
  assert.equal(Check(schema, { ...revise, findings: [{ ...revise.findings[0], toolCallIds: ['same', 'same'] }] }), false)
})

test('accepts camelCase or snake_case strategy input only when it matches the profile matrix', () => {
  const profile = resolveExecutionProfile({
    execution_profile: 'durable_v2',
    execution_strategy: {
      completion_audit: 'strict_v2',
      validation_policy: 'risk_v1',
      prompt_policy: 'stable_v1',
    },
  })
  assert.equal(profile.id, 'durable_v2')

  assert.throws(
    () => resolveExecutionProfile({
      id: 'durable_v2',
      strategy: { completionAudit: 'legacy' },
    }),
    /Unsupported execution strategy combination.*durable_v2 requires/,
  )
  assert.throws(() => resolveExecutionProfile('future_profile'), /Unsupported execution profile/)
  for (const inheritedKey of ['constructor', 'toString', '__proto__', 'hasOwnProperty']) {
    assert.throws(() => resolveExecutionProfile(inheritedKey), /Unsupported execution profile/)
  }
})

test('shadow and graph preview remove every side-effecting tool from the Runtime surface', () => {
  const tools = [
    { name: 'read' },
    { name: 'git_read' },
    { name: 'workflow_snapshot_get' },
    { name: 'workflow_start' },
    { name: 'workflow_stage_start' },
    { name: 'workflow_stage_complete' },
    { name: 'workflow_stage_fail' },
    { name: 'workflow_cancel' },
    { name: 'task_repair_escalate_start' },
    { name: 'write_file' },
    { name: 'run_command' },
    { name: 'web_search' },
    { name: 'goal_complete' },
    { name: 'child_run_start' },
    { name: 'continuation_propose' },
  ]
  for (const id of ['durable_v2_shadow', 'graph_readonly_preview']) {
    const selected = applyExecutionProfileToTools(tools, resolveExecutionProfile(id))
    assert.deepEqual(selected.tools.map(({ name }) => name), ['read', 'workflow_snapshot_get', 'continuation_propose'])
    assert.deepEqual(
      selected.excludedTools.map(({ name }) => name),
      [
        'git_read',
        'workflow_start',
        'workflow_stage_start',
        'workflow_stage_complete',
        'workflow_stage_fail',
        'workflow_cancel',
        'task_repair_escalate_start',
        'write_file',
        'run_command',
        'web_search',
        'goal_complete',
        'child_run_start',
      ],
    )
    assert.ok(selected.excludedTools
      .filter(({ name }) => name !== 'task_repair_escalate_start')
      .every((tool) => tool.reason === 'execution_profile'))
    assert.deepEqual(
      selected.excludedTools.find(({ name }) => name === 'task_repair_escalate_start'),
      {
        name: 'task_repair_escalate_start',
        reason: 'durable_v2_only_human_escalation',
        message: 'Human repair escalation is available only in durable_v2 and always requires a fresh Host approval bound to the current tool call.',
        profile: id,
      },
    )
  }

  const legacy = applyExecutionProfileToTools(tools, resolveExecutionProfile('legacy'))
  assert.deepEqual(
    legacy.tools.map(({ name }) => name),
    tools.filter(({ name }) => name !== 'task_repair_escalate_start').map(({ name }) => name),
  )
  assert.equal(legacy.excludedTools[0].reason, 'durable_v2_only_human_escalation')
  const durable = applyExecutionProfileToTools(tools, resolveExecutionProfile('durable_v2'))
  assert.deepEqual(
    durable.tools.map(({ name }) => name),
    ['read', 'git_read', 'workflow_snapshot_get', 'task_repair_escalate_start', 'write_file', 'run_command', 'web_search', 'goal_complete', 'child_run_start', 'continuation_propose'],
  )
  assert.deepEqual(
    durable.excludedTools.map(({ name }) => name),
    ['workflow_start', 'workflow_stage_start', 'workflow_stage_complete', 'workflow_stage_fail', 'workflow_cancel'],
  )
  assert.ok(durable.excludedTools.every(({ reason, message, profile }) => (
    reason === 'legacy_workflow_mutator_pending_attempt_api'
      && /unified Attempt\/Acceptance APIs/.test(message)
      && profile === 'durable_v2'
  )))
})

test('exposes graph_readonly_run only to the graph preview profile', () => {
  const tools = [{ name: 'graph_readonly_run' }]
  for (const id of ['legacy', 'durable_v2_shadow', 'durable_v2', 'graph_reviewer_v1']) {
    const selected = applyExecutionProfileToTools(tools, resolveExecutionProfile(id))
    assert.deepEqual(selected.tools, [])
    assert.deepEqual(selected.excludedTools.map(({ name }) => name), ['graph_readonly_run'])
  }
  const graph = applyExecutionProfileToTools(tools, resolveExecutionProfile('graph_readonly_preview'))
  assert.deepEqual(graph.tools.map(({ name }) => name), ['graph_readonly_run'])
  assert.deepEqual(graph.excludedTools, [])
})

test('exposes persistent read-only Graph tools only to durable_v2', () => {
  const tools = [
    { name: 'graph_readonly_activate' },
    { name: 'graph_readonly_snapshot_get' },
    { name: 'graph_readonly_node_start' },
    { name: 'graph_readonly_node_review' },
    { name: 'graph_readonly_node_finish' },
    { name: 'graph_readonly_node_cancel' },
    { name: 'graph_readonly_accept' },
  ]
  for (const id of ['legacy', 'durable_v2_shadow', 'graph_reviewer_v1', 'graph_readonly_preview']) {
    const selected = applyExecutionProfileToTools(tools, resolveExecutionProfile(id))
    assert.deepEqual(selected.tools, [])
    assert.deepEqual(
      selected.excludedTools,
      tools.map(({ name }) => ({
        name,
        reason: 'durable_v2_only_persistent_readonly_graph',
        message: 'Persistent read-only Graph activation, snapshots, node start, independent node review, criterion-bound node finish, bounded node cancel, and final Graph Acceptance are available only in durable_v2; graph_readonly_preview keeps its separate in-memory preview contract.',
        profile: id,
      })),
    )
  }
  const durable = applyExecutionProfileToTools(tools, resolveExecutionProfile('durable_v2'))
  assert.deepEqual(durable.tools, tools)
  assert.deepEqual(durable.excludedTools, [])
})

test('renders a short profile contract without changing the legacy prompt', () => {
  assert.equal(executionProfilePrompt(resolveExecutionProfile('legacy')), '')
  const shadow = executionProfilePrompt(resolveExecutionProfile('durable_v2_shadow'))
  assert.match(shadow, /ContinuationDecision is a Runtime proposal, never a Host fact/)
  assert.match(shadow, /read-only/)
  assert.match(shadow, /shadow comparison/)
  assert.doesNotMatch(shadow, /runId|eventCursor|decisionId/)
  const durable = executionProfilePrompt(resolveExecutionProfile('durable_v2'))
  assert.match(durable, /temporarily unavailable.*unified Attempt\/Acceptance APIs/)
  assert.match(durable, /workflow_snapshot_get remains available/)
  assert.match(durable, /graph_readonly_activate.*graph_readonly_snapshot_get/)
  assert.match(durable, /graph_readonly_node_start/)
  assert.match(durable, /Child completed is not a Reviewer pass.*Reviewer pass is not Node accepted.*Node accepted is not Goal accepted/)
  assert.match(durable, /current Attempt exactly once.*fresh, live, Host-valid Evidence/)
  assert.match(durable, /graph_readonly_node_review.*exact Task and Attempt versions.*criterionEvidence/)
  assert.match(durable, /current Lead.*implementation Child.*cannot review their own work/)
  assert.match(durable, /pass is valid only.*independent Reviewer.*allowlisted read-only ToolCall/)
  assert.match(durable, /Reviewer output is strict and Host-checked.*pass means every frozen criterion passed.*structured findings is empty/)
  assert.match(durable, /revise means at least one failed criterion.*bounded structured findings/)
  assert.match(durable, /graph_readonly_node_finish/)
  assert.match(durable, /Reviewer pass still does not accept the Node.*graph_readonly_snapshot_get.*graph_readonly_node_finish.*passed review request unchanged/)
  assert.match(durable, /same Goal, Task, Attempt and expected versions.*exactly the same current Attempt criterionEvidence and summary/)
  assert.match(durable, /every required Graph node is accepted.*current approved Plan.*no open review findings.*graph_readonly_accept/)
  assert.match(durable, /Generic acceptance_submit, goal_complete, and generic Attempt finish cannot accept or complete a Graph Goal/)
  assert.match(durable, /successful Graph tool result is not proof.*Host transition happened/)
  assert.match(durable, /graph_readonly_node_cancel/)
  assert.match(durable, /two-phase intent/)
  assert.match(durable, /Child completion wins a terminal race/)
  assert.match(durable, /authoritative Child terminal is completed.*fresh Evidence.*graph_readonly_node_finish/)
  assert.match(durable, /failed, cancelled, or interrupted.*Host reconciles that actual terminal/)
  const reviewer = executionProfilePrompt(resolveExecutionProfile('graph_reviewer_v1'))
  assert.match(reviewer, /independent high-risk Graph Reviewer, not the Lead and not the implementation Child/)
  assert.match(reviewer, /only read, ls, find, and grep/)
  assert.match(reviewer, /fresh evidence from a real allowlisted read-only ToolCall in this Reviewer Run/)
  assert.match(reviewer, /uncertainty or open finding must fail closed/)
  assert.match(reviewer, /exactly these four keys.*criteria.*recommendation.*summary.*findings/)
  assert.match(reviewer, /severity.*critical\|high\|medium\|low\|info/)
  assert.match(reviewer, /findings has at most 16 items.*title is 1 to 200.*detail is 1 to 2000/)
  assert.match(reviewer, /For pass.*every frozen criterion.*status passed.*findings must be empty/)
  assert.match(reviewer, /For revise.*at least one frozen criterion.*status failed.*findings must contain 1 to 16 items/)
  assert.match(reviewer, /finding criterion must exactly equal a frozen criterion whose status is failed/)
  assert.match(reviewer, /toolCallIds must be a subset of that criterion item's toolCallIds/)
  assert.match(reviewer, /For inconclusive, findings must be empty/)
  assert.match(reviewer, /Child completed is not Reviewer pass.*Reviewer pass is not Node accepted.*Node accepted is not Goal accepted/)
  assert.match(reviewer, /Tool success is not a Host state transition/)
  assert.match(reviewer, /This profile is read-only/)
  assert.doesNotMatch(reviewer, /Use continuation_propose/)
  const graph = executionProfilePrompt(resolveExecutionProfile('graph_readonly_preview'))
  assert.match(graph, /graph_readonly_run/)
  assert.match(graph, /at most 3 nodes and depth 1/)
  assert.doesNotMatch(graph, /scheduling is not implemented/)
})
