import { createHash } from 'node:crypto'
import { GRAPH_READONLY_TOOL_NAME } from './runtime-contract.mjs'

export const EXECUTION_PROFILE_SCHEMA_VERSION = 1

const TRIMMED_REQUIRED_STRING_PATTERN = '^(?![\\s\\u0085])[\\s\\S]*[^\\s\\u0085]$'

const reviewerString = (maxLength) => Object.freeze({
  type: 'string',
  minLength: 1,
  maxLength,
  pattern: TRIMMED_REQUIRED_STRING_PATTERN,
})

const reviewerToolCallIds = Object.freeze({
  type: 'array',
  minItems: 1,
  maxItems: 16,
  uniqueItems: true,
  items: reviewerString(200),
})

const reviewerCriterion = Object.freeze({
  type: 'object',
  additionalProperties: false,
  required: Object.freeze(['criterion', 'status', 'toolCallIds']),
  properties: Object.freeze({
    criterion: reviewerString(1_000),
    status: Object.freeze({ type: 'string', enum: Object.freeze(['passed', 'failed', 'inconclusive']) }),
    toolCallIds: reviewerToolCallIds,
  }),
})

const reviewerFinding = Object.freeze({
  type: 'object',
  additionalProperties: false,
  required: Object.freeze(['criterion', 'severity', 'title', 'detail', 'toolCallIds']),
  properties: Object.freeze({
    criterion: reviewerString(1_000),
    severity: Object.freeze({ type: 'string', enum: Object.freeze(['critical', 'high', 'medium', 'low', 'info']) }),
    title: reviewerString(200),
    detail: reviewerString(2_000),
    toolCallIds: reviewerToolCallIds,
  }),
})

export const GRAPH_REVIEWER_DECISION_OUTPUT_SCHEMA = Object.freeze({
  type: 'object',
  additionalProperties: false,
  required: Object.freeze(['criteria', 'recommendation', 'summary', 'findings']),
  properties: Object.freeze({
    criteria: Object.freeze({ type: 'array', minItems: 0, maxItems: 8, items: reviewerCriterion }),
    recommendation: Object.freeze({ type: 'string', enum: Object.freeze(['pass', 'revise', 'inconclusive']) }),
    summary: reviewerString(4_000),
    findings: Object.freeze({ type: 'array', minItems: 0, maxItems: 16, items: reviewerFinding }),
  }),
})

const STRATEGY_KEYS = Object.freeze([
  'completionAudit',
  'validationPolicy',
  'promptPolicy',
])

const READ_ONLY_SHADOW_TOOL_NAMES = new Set([
  'read',
  'ls',
  'find',
  'grep',
  'read_attachment',
  'sqlite_read',
  'structured_data',
  'tabular_data',
  'work_snapshot_get',
  'workflow_snapshot_get',
  'team_snapshot_get',
  'child_agent_list',
  'memory_search',
  'list_knowledge_bases',
  'search_knowledge',
  'read_knowledge_document',
  'query_knowledge_graph',
  'list_mcp_tools',
  'continuation_propose',
])

const GRAPH_REVIEWER_TOOL_NAMES = new Set([
  'read',
  'ls',
  'find',
  'grep',
])

const DURABLE_V2_LEGACY_WORKFLOW_MUTATORS = new Set([
  'workflow_start',
  'workflow_stage_start',
  'workflow_stage_complete',
  'workflow_stage_fail',
  'workflow_cancel',
])

const DURABLE_V2_WORKFLOW_EXCLUSION = Object.freeze({
  reason: 'legacy_workflow_mutator_pending_attempt_api',
  message: 'Legacy workflow mutation is temporarily unavailable in durable_v2 until it is integrated with the unified Attempt/Acceptance APIs.',
})

const DURABLE_V2_HUMAN_ESCALATION_TOOL_NAME = 'task_repair_escalate_start'
const DURABLE_V2_HUMAN_ESCALATION_EXCLUSION = Object.freeze({
  reason: 'durable_v2_only_human_escalation',
  message: 'Human repair escalation is available only in durable_v2 and always requires a fresh Host approval bound to the current tool call.',
})

const DURABLE_V2_PERSISTENT_GRAPH_TOOL_NAMES = new Set([
  'graph_readonly_activate',
  'graph_readonly_snapshot_get',
  'graph_readonly_node_start',
  'graph_readonly_node_review',
  'graph_readonly_node_finish',
  'graph_readonly_node_cancel',
  'graph_readonly_accept',
])
const DURABLE_V2_PERSISTENT_GRAPH_EXCLUSION = Object.freeze({
  reason: 'durable_v2_only_persistent_readonly_graph',
  message: 'Persistent read-only Graph activation, snapshots, node start, independent node review, criterion-bound node finish, bounded node cancel, and final Graph Acceptance are available only in durable_v2; graph_readonly_preview keeps its separate in-memory preview contract.',
})

const DEFINITIONS = Object.freeze({
  legacy: Object.freeze({
    id: 'legacy',
    strategies: Object.freeze({
      completionAudit: 'legacy',
      validationPolicy: 'legacy',
      promptPolicy: 'stable_v1',
    }),
    continuation: Object.freeze({ mode: 'disabled', schemaVersion: 1 }),
    graph: Object.freeze({ mode: 'disabled', maxNodes: 0, maxDepth: 0, writersAllowed: false }),
    toolPolicy: 'host_guarded',
    shadow: false,
  }),
  durable_v2_shadow: Object.freeze({
    id: 'durable_v2_shadow',
    strategies: Object.freeze({
      completionAudit: 'strict_v2',
      validationPolicy: 'risk_v1',
      promptPolicy: 'stable_v1',
    }),
    continuation: Object.freeze({ mode: 'shadow', schemaVersion: 1 }),
    graph: Object.freeze({ mode: 'disabled', maxNodes: 0, maxDepth: 0, writersAllowed: false }),
    toolPolicy: 'read_only_shadow',
    shadow: true,
  }),
  durable_v2: Object.freeze({
    id: 'durable_v2',
    strategies: Object.freeze({
      completionAudit: 'strict_v2',
      validationPolicy: 'risk_v1',
      promptPolicy: 'stable_v1',
    }),
    continuation: Object.freeze({ mode: 'propose', schemaVersion: 1 }),
    graph: Object.freeze({ mode: 'disabled', maxNodes: 0, maxDepth: 0, writersAllowed: false }),
    toolPolicy: 'host_guarded',
    shadow: false,
  }),
  graph_reviewer_v1: Object.freeze({
    id: 'graph_reviewer_v1',
    strategies: Object.freeze({
      completionAudit: 'strict_v2',
      validationPolicy: 'high_risk_v1',
      promptPolicy: 'graph_reviewer_v1',
    }),
    continuation: Object.freeze({ mode: 'disabled', schemaVersion: 1 }),
    graph: Object.freeze({ mode: 'disabled', maxNodes: 0, maxDepth: 0, writersAllowed: false }),
    toolPolicy: 'graph_reviewer_read_only',
    shadow: false,
  }),
  graph_readonly_preview: Object.freeze({
    id: 'graph_readonly_preview',
    strategies: Object.freeze({
      completionAudit: 'strict_v2',
      validationPolicy: 'risk_v1',
      promptPolicy: 'stable_v1',
    }),
    continuation: Object.freeze({ mode: 'propose', schemaVersion: 1 }),
    graph: Object.freeze({ mode: 'read_only_preview', maxNodes: 3, maxDepth: 1, writersAllowed: false }),
    toolPolicy: 'read_only_preview',
    shadow: false,
  }),
})

function strategyValue(strategy, camel, snake) {
  if (!strategy || typeof strategy !== 'object' || Array.isArray(strategy)) return undefined
  return strategy[camel] ?? strategy[snake]
}

function normalizeStrategy(strategy, fallback) {
  return {
    completionAudit: strategyValue(strategy, 'completionAudit', 'completion_audit') ?? fallback.completionAudit,
    validationPolicy: strategyValue(strategy, 'validationPolicy', 'validation_policy') ?? fallback.validationPolicy,
    promptPolicy: strategyValue(strategy, 'promptPolicy', 'prompt_policy') ?? fallback.promptPolicy,
  }
}

function stableObjectHash(value) {
  return createHash('sha256').update(JSON.stringify(value)).digest('hex').slice(0, 16)
}

function supportedCombinationMessage(definition) {
  const strategy = definition.strategies
  return `${definition.id} requires completionAudit=${strategy.completionAudit}, validationPolicy=${strategy.validationPolicy}, promptPolicy=${strategy.promptPolicy}`
}

export function supportedExecutionProfileIds() {
  return Object.keys(DEFINITIONS)
}

export function resolveExecutionProfile(input = 'legacy') {
  const config = typeof input === 'string'
    ? { id: input }
    : input && typeof input === 'object' && !Array.isArray(input)
      ? input
      : {}
  const id = String(config.id ?? config.executionProfile ?? config.execution_profile ?? 'legacy').trim()
  if (!Object.hasOwn(DEFINITIONS, id)) {
    throw new Error(`Unsupported execution profile: ${id || '<empty>'}. Supported profiles: ${supportedExecutionProfileIds().join(', ')}`)
  }
  const definition = DEFINITIONS[id]

  const suppliedStrategy = config.strategy ?? config.executionStrategy ?? config.execution_strategy
  const strategy = normalizeStrategy(suppliedStrategy, definition.strategies)
  const mismatch = STRATEGY_KEYS.find((key) => strategy[key] !== definition.strategies[key])
  if (mismatch) {
    throw new Error(`Unsupported execution strategy combination: ${supportedCombinationMessage(definition)}`)
  }

  return Object.freeze({
    schemaVersion: EXECUTION_PROFILE_SCHEMA_VERSION,
    id: definition.id,
    strategies: Object.freeze({ ...strategy }),
    continuation: definition.continuation,
    graph: definition.graph,
    toolPolicy: definition.toolPolicy,
    shadow: definition.shadow,
  })
}

export function executionProfileSnapshot(profile) {
  const resolved = profile?.schemaVersion === EXECUTION_PROFILE_SCHEMA_VERSION
    ? profile
    : resolveExecutionProfile(profile)
  const snapshot = {
    schemaVersion: EXECUTION_PROFILE_SCHEMA_VERSION,
    id: resolved.id,
    strategies: { ...resolved.strategies },
    continuation: {
      ...resolved.continuation,
      proposalOnly: resolved.continuation.mode !== 'disabled',
      hostValidationRequired: resolved.continuation.mode !== 'disabled',
    },
    graph: { ...resolved.graph },
    toolPolicy: resolved.toolPolicy,
    shadow: resolved.shadow,
    sideEffectsAllowed: resolved.toolPolicy === 'host_guarded',
  }
  return Object.freeze({ ...snapshot, hash: stableObjectHash(snapshot) })
}

export function executionProfileAllowsTool(profile, toolName) {
  const resolved = profile?.schemaVersion === EXECUTION_PROFILE_SCHEMA_VERSION
    ? profile
    : resolveExecutionProfile(profile)
  if (resolved.id === 'graph_reviewer_v1') return GRAPH_REVIEWER_TOOL_NAMES.has(toolName)
  if (toolName === GRAPH_READONLY_TOOL_NAME) return resolved.graph.mode === 'read_only_preview'
  if (DURABLE_V2_PERSISTENT_GRAPH_TOOL_NAMES.has(toolName)) return resolved.id === 'durable_v2'
  if (toolName === DURABLE_V2_HUMAN_ESCALATION_TOOL_NAME) return resolved.id === 'durable_v2'
  if (resolved.id === 'durable_v2' && DURABLE_V2_LEGACY_WORKFLOW_MUTATORS.has(toolName)) return false
  if (resolved.toolPolicy === 'host_guarded') return true
  return READ_ONLY_SHADOW_TOOL_NAMES.has(toolName)
}

export function applyExecutionProfileToTools(tools, profile) {
  const resolved = profile?.schemaVersion === EXECUTION_PROFILE_SCHEMA_VERSION
    ? profile
    : resolveExecutionProfile(profile)
  const allowed = []
  const excluded = []
  for (const tool of Array.isArray(tools) ? tools : []) {
    if (executionProfileAllowsTool(resolved, tool?.name)) allowed.push(tool)
    else if (tool?.name) {
      const diagnostic = DURABLE_V2_PERSISTENT_GRAPH_TOOL_NAMES.has(tool.name)
        ? DURABLE_V2_PERSISTENT_GRAPH_EXCLUSION
        : tool.name === DURABLE_V2_HUMAN_ESCALATION_TOOL_NAME
          ? DURABLE_V2_HUMAN_ESCALATION_EXCLUSION
        : resolved.id === 'durable_v2' && DURABLE_V2_LEGACY_WORKFLOW_MUTATORS.has(tool.name)
          ? DURABLE_V2_WORKFLOW_EXCLUSION
          : { reason: 'execution_profile' }
      excluded.push({ name: tool.name, ...diagnostic, profile: resolved.id })
    }
  }
  return { tools: allowed, excludedTools: excluded }
}

export function executionProfileCatalogTools(catalog, profile) {
  return applyExecutionProfileToTools(catalog, profile).tools.map((tool) => ({ ...tool }))
}

export function executionProfilePrompt(profile) {
  const resolved = profile?.schemaVersion === EXECUTION_PROFILE_SCHEMA_VERSION
    ? profile
    : resolveExecutionProfile(profile)
  if (resolved.id === 'legacy') return ''

  const lines = [`<execution_profile id="${resolved.id}" schema_version="${EXECUTION_PROFILE_SCHEMA_VERSION}">`]
  if (resolved.id === 'graph_reviewer_v1') {
    lines.push('You are an independent high-risk Graph Reviewer, not the Lead and not the implementation Child. Review only the current frozen acceptance criteria and the Host-supplied evidence bindings for this review request.')
    lines.push('Use only read, ls, find, and grep to obtain fresh read-only proof. Do not call Graph, work, delegation, approval, continuation, process, write, memory, knowledge, attachment, or remote tools; this hidden profile does not expose them.')
    lines.push('A Child report and model prose are not proof. A pass requires fresh evidence from a real allowlisted read-only ToolCall in this Reviewer Run, and any uncertainty or open finding must fail closed instead of approving.')
    lines.push('Return exactly one strict JSON object with exactly these four keys and no Markdown or extra keys: {"criteria":[{"criterion":"...","status":"passed|failed|inconclusive","toolCallIds":["..."]}],"recommendation":"pass|revise|inconclusive","summary":"...","findings":[{"criterion":"...","severity":"critical|high|medium|low|info","title":"...","detail":"...","toolCallIds":["..."]}]}. criteria has at most 8 items; each criterion is at most 1000 characters and each item has 1 to 16 unique ToolCall IDs. summary is 1 to 4000 trimmed characters. findings has at most 16 items; title is 1 to 200 trimmed characters and detail is 1 to 2000 trimmed characters; finding ToolCall IDs are also 1 to 16 and unique within the item.')
    lines.push('For pass, every frozen criterion must appear in frozen order with status passed and findings must be empty. For revise, at least one frozen criterion must have status failed and findings must contain 1 to 16 items. Each finding criterion must exactly equal a frozen criterion whose status is failed, and its toolCallIds must be a subset of that criterion item\'s toolCallIds. For inconclusive, findings must be empty. Never invent criterion text or proof IDs.')
    lines.push('Child completed is not Reviewer pass; Reviewer pass is not Node accepted; Node accepted is not Goal accepted. Tool success is not a Host state transition.')
  } else {
    lines.push('ContinuationDecision is a Runtime proposal, never a Host fact. Use continuation_propose once at a genuine continue, repair, approval-wait, completion-candidate, or blocking decision point.')
    lines.push('The Host alone validates, persists, and applies Goal, Task, Evidence, Acceptance, Approval, blocked, and completion state. A successful proposal tool result does not mean the proposed transition was accepted.')
  }
  if (resolved.id === 'durable_v2') {
    lines.push('Legacy expert Workflow mutation tools are temporarily unavailable until they are integrated with the unified Attempt/Acceptance APIs. workflow_snapshot_get remains available for inspection; direct or replayed mutation requests are still rejected by the Host.')
    lines.push('For an approved bounded read-only Graph PlanRevision, use graph_readonly_activate once, then graph_readonly_snapshot_get to inspect Host-derived readiness. Start only a runnable queued node with graph_readonly_node_start and the snapshot\'s current Task version.')
    lines.push('Keep all four states separate: Child completed is not a Reviewer pass; a Reviewer pass is not Node accepted; Node accepted is not Goal accepted. Never turn a Child report, Reviewer prose, or your own claim into a Host state transition.')
    lines.push('After the Child completes, bind every frozen criterion for the current Attempt exactly once in snapshot order to fresh, live, Host-valid Evidence. Standard-risk nodes may then use graph_readonly_node_finish. For a high-risk node, first call graph_readonly_node_review with the snapshot\'s exact Task and Attempt versions and those criterionEvidence bindings. The current Lead, the implementation Child, and any Run that implemented the Attempt cannot review their own work. A pass is valid only when the independent Reviewer produced fresh proof with a real allowlisted read-only ToolCall during this review; model prose and old review output are not proof.')
    lines.push('The Reviewer output is strict and Host-checked: pass means every frozen criterion passed and structured findings is empty; revise means at least one failed criterion and one or more bounded structured findings. The Lead must not invent, rewrite, or discard Reviewer findings.')
    lines.push('A Reviewer pass still does not accept the Node. Re-read graph_readonly_snapshot_get, then call graph_readonly_node_finish with the passed review request unchanged: use the same Goal, Task, Attempt and expected versions, and exactly the same current Attempt criterionEvidence and summary. Only after the snapshot proves every required Graph node is accepted under the current approved Plan and there are no open review findings may you call graph_readonly_accept. Generic acceptance_submit, goal_complete, and generic Attempt finish cannot accept or complete a Graph Goal.')
    lines.push('A successful Graph tool result is not proof that the Host transition happened. Re-read graph_readonly_snapshot_get and the Host-owned Goal state before claiming review, Node acceptance, or Goal acceptance.')
    lines.push('Use graph_readonly_node_cancel only for a running standard-risk Graph Attempt with the snapshot\'s exact Task and Attempt versions; cancellation is a Host-owned two-phase intent, Child completion wins a terminal race, and a successful tool result does not itself prove the Child is cancelled. If the authoritative Child terminal is completed, continue through fresh Evidence and graph_readonly_node_finish; if it is failed, cancelled, or interrupted, the Host reconciles that actual terminal into the Graph Attempt and Task, so do not claim acceptance or submit a model-chosen terminal status.')
  }
  if (resolved.toolPolicy !== 'host_guarded') {
    lines.push('This profile is read-only. Do not write files, run processes, start workflows or child runs, mutate work or memory state, send messages, call remote action tools, or otherwise cause side effects.')
  }
  if (resolved.continuation.mode === 'shadow') {
    lines.push('This is a shadow comparison. Proposals are observation-only and must not alter the visible legacy state transition.')
  }
  if (resolved.graph.mode === 'read_only_preview') {
    lines.push('Use graph_readonly_run only when independent read-only inspection agents materially help. It accepts at most 3 nodes and depth 1, exposes only read/ls/find/grep inside each node, and never creates durable Task, Child Run, Evidence, Acceptance, or Writer state.')
  }
  lines.push('</execution_profile>')
  return lines.join('\n')
}
