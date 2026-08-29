import { createHash } from 'node:crypto'
import modelMatrix from '../evals/fixtures/model-adapter-matrix.json' with { type: 'json' }
import toolContract from '../evals/fixtures/bfcl-tool-contract.json' with { type: 'json' }
import injectionBoundary from '../evals/fixtures/agentdojo-injection.json' with { type: 'json' }
import intentRegressions from '../evals/fixtures/fox-intent-regressions.json' with { type: 'json' }
import codingAgentContract from '../evals/fixtures/swe-bench-agent-contract.json' with { type: 'json' }
import recoveryContract from '../evals/fixtures/fox-recovery-contract.json' with { type: 'json' }
import permissionFailureContract from '../evals/fixtures/fox-permission-failure-contract.json' with { type: 'json' }
import graphReviewAcceptanceContract from '../evals/fixtures/fox-graph-review-acceptance-contract.json' with { type: 'json' }
import quickBaselineManifest from '../evals/manifests/phase-0a-fast-baseline.json' with { type: 'json' }
import fullBaselineManifest from '../evals/manifests/phase-0b-full-baseline.json' with { type: 'json' }
import { resolveModelProfile } from './model-profile.mjs'
import { shouldUsePlanner } from './planner-runtime.mjs'
import { composeRuntimePrompt, FOX_RUNTIME_INSTRUCTIONS } from './runtime-instructions.mjs'
import { createCapabilityManifest, RUNTIME_TOOL_CATALOG, validateCapabilityManifest } from './runtime-contract.mjs'
import {
  applyExecutionProfileToTools,
  executionProfilePrompt,
  GRAPH_REVIEWER_DECISION_OUTPUT_SCHEMA,
  resolveExecutionProfile,
} from './execution-profile.mjs'
import { createHostTools } from './host-tools.mjs'
import { resolvePromptPolicy } from './prompt-registry.mjs'
import { sanitizeProviderHistory, transcriptFromSession } from './runtime-session.mjs'

const DATASET_VERSION = 'fox-phase0b-2026-08-27'
const EVALUATOR_VERSION = 'fox-offline-evaluator-v8'
const CATEGORIES = ['simple', 'medium', 'complex', 'recovery']

function canonicalJson(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value)
  if (Array.isArray(value)) return `[${value.map((item) => canonicalJson(item)).join(',')}]`
  return `{${Object.keys(value)
    .filter((key) => value[key] !== undefined)
    .sort((left, right) => left.localeCompare(right))
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`
}

function hashJson(value) {
  return createHash('sha256').update(canonicalJson(value)).digest('hex')
}

function result(id, passed, details = {}) {
  return { id, passed: Boolean(passed), ...details }
}

function matches(actual, expected) {
  return Object.entries(expected).every(([key, value]) => actual[key] === value)
}

function evaluateModelMatrix() {
  const cases = modelMatrix.cases.map((item) => {
    const profile = resolveModelProfile(item.config)
    return result(item.id, matches(profile, item.expected), {
      expected: item.expected,
      actual: Object.fromEntries(Object.keys(item.expected).map((key) => [key, profile[key]])),
    })
  })
  return { id: 'model-adapter-matrix', name: modelMatrix.name, scope: modelMatrix.description, cases }
}

function evaluateToolContract() {
  const catalog = new Map(RUNTIME_TOOL_CATALOG.map((tool) => [tool.name, tool]))
  const cases = toolContract.cases.map((item) => {
    const actual = catalog.get(item.expectedTool)
    const expected = {
      name: item.expectedTool,
      category: item.category,
      execution: item.execution,
      approval: item.approval,
    }
    return result(item.id, Boolean(actual) && matches(actual, expected), { expected, actual: actual ?? null })
  })
  return { id: 'bfcl-tool-contract', name: toolContract.name, source: toolContract.source, sourceUrl: toolContract.sourceUrl, scope: toolContract.scope, cases }
}

function evaluateCodingAgentContract() {
  const catalog = new Map(RUNTIME_TOOL_CATALOG.map((tool) => [tool.name, tool]))
  const cases = codingAgentContract.cases.map((item) => {
    const missingTools = item.requiredTools.filter((name) => !catalog.has(name))
    const unsafeMutations = item.requiredTools
      .map((name) => catalog.get(name))
      .filter((tool) => tool && ['project-write', 'process'].includes(tool.category) && tool.approval === 'none')
      .map((tool) => tool.name)
    return result(item.id, missingTools.length === 0 && unsafeMutations.length === 0, {
      requiredTools: item.requiredTools,
      missingTools,
      unsafeMutations,
    })
  })
  return {
    id: 'coding-agent-contract',
    name: codingAgentContract.name,
    source: codingAgentContract.source,
    sourceUrl: codingAgentContract.sourceUrl,
    scope: codingAgentContract.scope,
    cases,
  }
}

function evaluateInjectionBoundary() {
  const baseline = composeRuntimePrompt({
    systemPrompt: 'You are Fox.',
    context: { projectRoot: 'D:/authorized-project', workSnapshot: { goal: { id: 'eval-goal', title: '' } } },
    turn: { date: '2026-08-06T00:00:00.000Z' },
  })
  const contractPresent = /untrusted data/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /Never reveal API keys/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /approval boundary/.test(FOX_RUNTIME_INSTRUCTIONS)
  const cases = injectionBoundary.cases.map((item) => {
    const composed = composeRuntimePrompt({
      systemPrompt: 'You are Fox.',
      context: {
        projectRoot: 'D:/authorized-project',
        workSnapshot: { goal: { id: 'eval-goal', title: item.payload } },
      },
      turn: { date: '2026-08-06T00:00:00.000Z' },
    })
    const boundary = '<fox_context_block kind="work_snapshot" authority="host">'
    const workSnapshotStart = composed.prompt.indexOf(boundary)
    const workSnapshotEnd = composed.prompt.indexOf('</fox_context_block>', workSnapshotStart)
    const payloadStart = composed.prompt.indexOf(item.payload)
    const authorityCount = composed.prompt.split(boundary).length - 1
    const isolated = authorityCount === 1
      && workSnapshotStart >= 0
      && workSnapshotEnd > workSnapshotStart
      && payloadStart > workSnapshotStart
      && payloadStart < workSnapshotEnd
    return result(item.id, contractPresent && isolated && composed.stablePromptHash === baseline.stablePromptHash, {
      stablePromptUnchanged: composed.stablePromptHash === baseline.stablePromptHash,
      isolatedInHostWorkSnapshot: isolated,
      workSnapshotAuthorityCount: authorityCount,
      safetyContractPresent: contractPresent,
    })
  })
  return { id: 'injection-boundary', name: injectionBoundary.name, source: injectionBoundary.source, sourceUrl: injectionBoundary.sourceUrl, scope: injectionBoundary.scope, cases }
}

function evaluateIntentRegressions() {
  const cases = intentRegressions.cases.map((item) => {
    const actual = shouldUsePlanner(item.text, {
      hasProject: true,
      approvalDemo: item.approvalDemo === true,
    })
    return result(item.id, actual === item.expectedPlanner, { expected: item.expectedPlanner, actual })
  })
  const goalContract = /only when the user's latest message explicitly asks Fox to create or track a Goal/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /Do not infer Goal intent from pasted or quoted content/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /Only the Fox Host can activate/.test(FOX_RUNTIME_INSTRUCTIONS)
  cases.push(result('host-owned-goal-contract', goalContract))
  return { id: 'fox-intent-regressions', name: intentRegressions.name, scope: intentRegressions.scope, cases }
}

function evaluateRecoveryScenario(scenario) {
  if (scenario === 'tool-result-after-interruption') {
    const restored = sanitizeProviderHistory([
      { role: 'assistant', content: [{ type: 'toolCall', id: 'call-1', name: 'run_command', arguments: { cmd: 'test' } }] },
      { role: 'toolResult', toolCallId: 'call-1', toolName: 'run_command', content: [{ type: 'text', text: 'exit=0' }, { type: 'image', data: 'ignored' }], details: { namespace: 'private' }, isError: false },
    ])
    return restored.length === 2
      && restored[0].content?.[0]?.id === 'call-1'
      && restored[1].content?.length === 1
      && restored[1].content[0].text === 'exit=0'
      && Object.keys(restored[1].details ?? {}).length === 0
  }
  if (scenario === 'validation-state-after-interruption') {
    const fallback = [
      { role: 'user', content: 'run validation' },
      { role: 'assistant', content: 'validation pending' },
    ]
    const restored = transcriptFromSession({ messages: fallback }, fallback)
    return restored.length === 2
      && restored[1].role === 'assistant'
      && restored[1].content?.[0]?.text === 'validation pending'
  }
  if (scenario === 'waiting-approval-authoritative-fallback') {
    const fallback = [{ role: 'assistant', content: 'approval denied by Host' }]
    const restored = transcriptFromSession({ messages: [{ role: 'assistant', content: 'approval pending' }] }, fallback)
    return JSON.stringify(restored) === JSON.stringify(fallback)
  }
  if (scenario === 'child-run-tool-state-after-interruption') {
    const restored = sanitizeProviderHistory([
      { role: 'assistant', content: [{ type: 'toolCall', id: 'child-1', name: 'child_run_wait', arguments: { childRunId: 'child-1' } }] },
      { role: 'toolResult', toolCallId: 'child-1', toolName: 'child_run_wait', content: 'child still running', details: { internalTrace: 'hidden' }, isError: false },
    ])
    return restored[0]?.content?.[0]?.name === 'child_run_wait'
      && restored[1]?.toolCallId === 'child-1'
      && restored[1]?.content === 'child still running'
      && Object.keys(restored[1]?.details ?? {}).length === 0
  }
  if (scenario === 'missing-session-authoritative-fallback') {
    const fallback = [{ role: 'user', content: 'continue from authoritative work state' }]
    return JSON.stringify(transcriptFromSession(undefined, fallback)) === JSON.stringify(fallback)
  }
  return false
}

function evaluateRecoveryContract() {
  const cases = recoveryContract.cases.map((item) => result(item.id, evaluateRecoveryScenario(item.scenario), {
    scenario: item.scenario,
    boundary: 'runtime-session',
  }))
  return { id: 'runtime-recovery-contract', name: recoveryContract.name, scope: recoveryContract.scope, cases }
}

function evaluateRuntimeToolCatalog() {
  const allowedCategories = new Set([
    'project-read',
    'project-write',
    'attachment',
    'process',
    'knowledge',
    'skill',
    'mcp',
    'work',
    'memory',
    'delegation',
  ])
  const allowedExecutions = new Set(['runtime', 'host', 'remote'])
  const allowedApprovals = new Set(['none', 'preflight', 'policy', 'always'])
  const names = RUNTIME_TOOL_CATALOG.map((tool) => tool.name)
  const uniqueNames = new Set(names)
  const cases = RUNTIME_TOOL_CATALOG.map((tool) => {
    const metadataValid = typeof tool.name === 'string'
      && tool.name.length > 0
      && allowedCategories.has(tool.category)
      && allowedExecutions.has(tool.execution)
      && allowedApprovals.has(tool.approval)
    const protectedMutation = !['project-write', 'process'].includes(tool.category) || tool.approval !== 'none'
    const runtimeIsReadOnly = tool.execution !== 'runtime' || tool.category === 'project-read'
    return result(tool.name, metadataValid && protectedMutation && runtimeIsReadOnly && uniqueNames.size === names.length, {
      expected: {
        uniqueName: true,
        protectedMutation: true,
        runtimeIsReadOnly: true,
      },
      actual: {
        name: tool.name,
        category: tool.category,
        execution: tool.execution,
        approval: tool.approval,
        uniqueName: uniqueNames.size === names.length,
        protectedMutation,
        runtimeIsReadOnly,
      },
    })
  })
  return {
    id: 'runtime-tool-catalog',
    name: 'Fox Runtime tool catalog coverage',
    scope: 'One deterministic metadata and permission-boundary check for every current RUNTIME_TOOL_CATALOG entry.',
    cases,
  }
}

function exactSchemaKeys(schema, expectedKeys) {
  return schema?.additionalProperties === false
    && canonicalJson(Object.keys(schema.properties ?? {})) === canonicalJson(expectedKeys)
    && canonicalJson(schema.required ?? []) === canonicalJson(expectedKeys)
}

function visibleOnlyInDurableV2(toolName) {
  const tool = { name: toolName }
  const durable = applyExecutionProfileToTools([tool], resolveExecutionProfile('durable_v2'))
  if (durable.tools.length !== 1 || durable.excludedTools.length !== 0) return false
  return ['legacy', 'durable_v2_shadow', 'graph_reviewer_v1', 'graph_readonly_preview'].every((profileId) => {
    const selected = applyExecutionProfileToTools([tool], resolveExecutionProfile(profileId))
    return selected.tools.length === 0
      && selected.excludedTools.length === 1
      && selected.excludedTools[0].name === toolName
      && selected.excludedTools[0].reason === 'durable_v2_only_persistent_readonly_graph'
  })
}

function graphReviewerProfileIsBounded() {
  const profile = resolveExecutionProfile('graph_reviewer_v1')
  const selected = applyExecutionProfileToTools(RUNTIME_TOOL_CATALOG, profile)
  const prompt = executionProfilePrompt(profile)
  const schema = GRAPH_REVIEWER_DECISION_OUTPUT_SCHEMA
  const criterion = schema.properties?.criteria?.items
  const finding = schema.properties?.findings?.items
  return {
    strictHighRiskPolicy: profile.strategies.completionAudit === 'strict_v2'
      && profile.strategies.validationPolicy === 'high_risk_v1'
      && profile.strategies.promptPolicy === 'graph_reviewer_v1'
      && resolvePromptPolicy({ profileId: profile.id, promptPolicy: profile.strategies.promptPolicy }).policy === 'graph_reviewer_v1',
    continuationDisabled: profile.continuation.mode === 'disabled',
    graphDisabled: profile.graph.mode === 'disabled' && profile.graph.writersAllowed === false,
    exactProofTools: canonicalJson(selected.tools.map(({ name }) => name)) === canonicalJson(['read', 'ls', 'find', 'grep']),
    noOrchestrationTools: selected.tools.every(({ category }) => !['work', 'delegation'].includes(category)),
    independentReviewerPrompt: /independent high-risk Graph Reviewer, not the Lead and not the implementation Child/.test(prompt),
    freshAllowlistedProof: /fresh evidence from a real allowlisted read-only ToolCall in this Reviewer Run/.test(prompt),
    failClosed: /uncertainty or open finding must fail closed/.test(prompt),
    exactFourKeyOutput: schema.additionalProperties === false
      && canonicalJson(schema.required) === canonicalJson(['criteria', 'recommendation', 'summary', 'findings'])
      && canonicalJson(Object.keys(schema.properties ?? {})) === canonicalJson(['criteria', 'recommendation', 'summary', 'findings'])
      && schema.properties.criteria.minItems === 0
      && schema.properties.criteria.maxItems === 8
      && criterion?.additionalProperties === false
      && canonicalJson(criterion?.required) === canonicalJson(['criterion', 'status', 'toolCallIds'])
      && canonicalJson(criterion?.properties?.status?.enum) === canonicalJson(['passed', 'failed', 'inconclusive'])
      && criterion?.properties?.criterion?.maxLength === 1_000
      && criterion?.properties?.toolCallIds?.minItems === 1
      && criterion?.properties?.toolCallIds?.maxItems === 16
      && criterion?.properties?.toolCallIds?.uniqueItems === true
      && schema.properties.summary.maxLength === 4_000,
    boundedStructuredFindings: schema.properties.findings.minItems === 0
      && schema.properties.findings.maxItems === 16
      && finding?.additionalProperties === false
      && canonicalJson(finding?.required) === canonicalJson(['criterion', 'severity', 'title', 'detail', 'toolCallIds'])
      && canonicalJson(finding?.properties?.severity?.enum) === canonicalJson(['critical', 'high', 'medium', 'low', 'info'])
      && finding?.properties?.criterion?.maxLength === 1_000
      && finding?.properties?.title?.maxLength === 200
      && finding?.properties?.detail?.maxLength === 2_000
      && finding?.properties?.toolCallIds?.minItems === 1
      && finding?.properties?.toolCallIds?.maxItems === 16
      && finding?.properties?.toolCallIds?.uniqueItems === true,
    outcomeFindingRules: /For pass, every frozen criterion must appear in frozen order with status passed and findings must be empty/.test(prompt)
      && /For revise, at least one frozen criterion must have status failed and findings must contain 1 to 16 items/.test(prompt)
      && /Each finding criterion must exactly equal a frozen criterion whose status is failed/.test(prompt)
      && /toolCallIds must be a subset of that criterion item's toolCallIds/.test(prompt)
      && /For inconclusive, findings must be empty/.test(prompt),
  }
}

function evaluateGraphReviewAcceptanceScenario(item, toolsByName, prompt) {
  const scenario = item?.scenario
  const review = toolsByName.get('graph_readonly_node_review')
  const finish = toolsByName.get('graph_readonly_node_finish')
  const accept = toolsByName.get('graph_readonly_accept')
  if (scenario === 'node-review-independent-current-attempt') {
    const reviewerContract = graphReviewerProfileIsBounded()
    const expectedKeys = [
      'goalId',
      'taskId',
      'attemptId',
      'expectedTaskVersion',
      'expectedAttemptVersion',
      'criterionEvidence',
      'summary',
    ]
    return {
      exactReviewSchema: exactSchemaKeys(review?.parameters, expectedKeys),
      finishBoundsReused: Boolean(review && finish)
        && canonicalJson(review.parameters) === canonicalJson(finish.parameters),
      durableV2Only: visibleOnlyInDurableV2('graph_readonly_node_review'),
      currentAttemptFreshEvidence: /current Attempt exactly once in snapshot order to fresh, live, Host-valid Evidence/.test(prompt),
      independentReviewer: /current Lead, the implementation Child, and any Run that implemented the Attempt cannot review their own work/.test(prompt),
      allowlistedToolProof: /pass is valid only when the independent Reviewer produced fresh proof with a real allowlisted read-only ToolCall/.test(prompt),
      boundedReviewerProfile: [
        'strictHighRiskPolicy',
        'continuationDisabled',
        'graphDisabled',
        'exactProofTools',
        'noOrchestrationTools',
        'independentReviewerPrompt',
        'freshAllowlistedProof',
        'failClosed',
      ].every((key) => reviewerContract[key] === true),
      exactFourKeyOutput: reviewerContract.exactFourKeyOutput,
      boundedStructuredFindings: reviewerContract.boundedStructuredFindings,
      outcomeFindingRules: reviewerContract.outcomeFindingRules,
      fixtureRequiresStructuredFindings: item?.requiresStructuredFindings === true,
    }
  }
  if (scenario === 'separate-node-review-and-goal-acceptance') {
    return {
      fourDistinctStates: /Child completed is not a Reviewer pass; a Reviewer pass is not Node accepted; Node accepted is not Goal accepted/.test(prompt),
      passStillNeedsFinish: /Reviewer pass still does not accept the Node/.test(prompt)
        && /call graph_readonly_node_finish with the passed review request unchanged/.test(prompt)
        && /exactly the same current Attempt criterionEvidence and summary/.test(prompt),
      toolResultNotTransition: /successful Graph tool result is not proof that the Host transition happened/.test(prompt),
      genericBypassClosed: /Generic acceptance_submit, goal_complete, and generic Attempt finish cannot accept or complete a Graph Goal/.test(prompt),
    }
  }
  if (scenario === 'final-current-plan-clean-findings-only') {
    return {
      exactAcceptSchema: exactSchemaKeys(accept?.parameters, ['goalId', 'expectedGoalVersion', 'summary']),
      durableV2Only: visibleOnlyInDurableV2('graph_readonly_accept'),
      allNodesAccepted: /every required Graph node is accepted/.test(prompt),
      currentPlan: /under the current approved Plan/.test(prompt),
      noOpenFindings: /there are no open review findings/.test(prompt),
      dedicatedAcceptance: /call graph_readonly_accept/.test(prompt),
    }
  }
  return { knownScenario: false }
}

function evaluateGraphReviewAcceptanceContract() {
  const toolsByName = new Map(createHostTools(() => {}).map((tool) => [tool.name, tool]))
  const prompt = executionProfilePrompt(resolveExecutionProfile('durable_v2'))
  const cases = graphReviewAcceptanceContract.cases.map((item) => {
    const checks = evaluateGraphReviewAcceptanceScenario(item, toolsByName, prompt)
    return result(item.id, Object.values(checks).every(Boolean), { scenario: item.scenario, checks })
  })
  return {
    id: 'graph-review-acceptance-contract',
    name: graphReviewAcceptanceContract.name,
    scope: graphReviewAcceptanceContract.scope,
    cases,
  }
}

function permissionManifestForScenario(scenario) {
  const baseline = createCapabilityManifest()
  const firstTool = baseline.tools[0]
  if (scenario === 'non-object') return null
  if (scenario === 'unsupported-version') return { ...baseline, manifestVersion: 999 }
  if (scenario === 'non-boolean-tool-approval') return { ...baseline, toolApproval: 'yes' }
  if (scenario === 'non-array-tools') return { ...baseline, tools: {} }
  if (scenario === 'non-object-tool-entry') return { ...baseline, tools: [null] }
  if (scenario === 'missing-tool-name') return { ...baseline, tools: [{ ...firstTool, name: '' }] }
  if (scenario === 'unsupported-tool-category') return { ...baseline, tools: [{ ...firstTool, category: 'unsafe' }] }
  if (scenario === 'unsupported-tool-execution') return { ...baseline, tools: [{ ...firstTool, execution: 'direct' }] }
  if (scenario === 'unsupported-tool-approval') return { ...baseline, tools: [{ ...firstTool, approval: 'implicit' }] }
  if (scenario === 'work-without-work-loop') {
    const workTool = baseline.tools.find((tool) => tool.category === 'work')
    return { ...baseline, workLoop: false, tools: [workTool] }
  }
  if (scenario === 'duplicate-tool-name') return { ...baseline, tools: [firstTool, { ...firstTool }] }
  if (scenario === 'valid-v2') return baseline
  if (scenario === 'valid-v1-without-work-tools') {
    const legacy = createCapabilityManifest({ workLoop: false })
    delete legacy.workLoop
    legacy.manifestVersion = 1
    return legacy
  }
  throw new Error(`Unknown permission/failure Eval scenario: ${scenario}`)
}

function evaluatePermissionFailureContract() {
  const cases = permissionFailureContract.cases.map((item) => {
    const actualError = validateCapabilityManifest(permissionManifestForScenario(item.scenario))
    return result(item.id, actualError === item.expectedError, {
      scenario: item.scenario,
      expectedError: item.expectedError,
      actualError,
    })
  })
  return {
    id: 'permission-failure-contract',
    name: permissionFailureContract.name,
    scope: permissionFailureContract.scope,
    cases,
  }
}

function suiteCaseMap(suites) {
  return new Map(suites.flatMap((suite) => suite.cases.map((item) => [`${suite.id}/${item.id}`, { suite, item }])))
}

export function resolveEvaluationManifest(manifest = 'full') {
  if (manifest === 'full' || manifest === fullBaselineManifest.id || manifest === undefined) return fullBaselineManifest
  if (manifest === 'fast' || manifest === quickBaselineManifest.id) return quickBaselineManifest
  if (manifest && typeof manifest === 'object') return manifest
  throw new Error(`Unknown Fox offline Eval manifest: ${String(manifest)}`)
}

export function validateBaselineManifest(manifest, suites) {
  if (manifest?.kind !== 'fox-offline-eval-manifest' || manifest?.schemaVersion !== 1) {
    throw new Error('Unsupported Fox offline Eval manifest.')
  }
  const available = suiteCaseMap(suites)
  const seen = new Set()
  const counts = Object.fromEntries(CATEGORIES.map((category) => [category, 0]))
  for (const entry of manifest.cases ?? []) {
    const reference = `${entry.suiteId}/${entry.caseId}`
    if (seen.has(reference)) throw new Error(`Duplicate offline Eval manifest case: ${reference}`)
    if (!available.has(reference)) throw new Error(`Unknown offline Eval manifest case: ${reference}`)
    if (!CATEGORIES.includes(entry.category)) throw new Error(`Unknown offline Eval category: ${entry.category}`)
    seen.add(reference)
    counts[entry.category] += 1
  }
  const expected = manifest.expectedCaseCounts ?? {}
  for (const category of CATEGORIES) {
    if (counts[category] !== expected[category]) {
      throw new Error(`Offline Eval manifest category ${category} expected ${expected[category]} cases, found ${counts[category]}.`)
    }
  }
  if (seen.size !== expected.total) {
    throw new Error(`Offline Eval manifest expected ${expected.total} total cases, found ${seen.size}.`)
  }

  if (manifest.coverage?.runtimeToolCatalog === 'all') {
    const expectedToolReferences = RUNTIME_TOOL_CATALOG.map((tool) => `runtime-tool-catalog/${tool.name}`)
    const missingTools = expectedToolReferences.filter((reference) => !seen.has(reference))
    const selectedToolReferences = [...seen].filter((reference) => reference.startsWith('runtime-tool-catalog/'))
    if (missingTools.length > 0 || selectedToolReferences.length !== expectedToolReferences.length) {
      throw new Error(`Offline Eval manifest does not cover the complete Runtime tool catalog; missing: ${missingTools.join(', ') || 'none'}.`)
    }
    const actualCategories = [...new Set(RUNTIME_TOOL_CATALOG.map((tool) => tool.category))].sort()
    const requiredCategories = [...(manifest.coverage.requiredCategories ?? [])].sort()
    if (canonicalJson(actualCategories) !== canonicalJson(requiredCategories)) {
      throw new Error(`Offline Eval manifest Runtime categories differ from the catalog: ${actualCategories.join(', ')}.`)
    }
  }

  for (const gate of manifest.hardGates ?? []) {
    const references = new Set(gate.caseRefs ?? [])
    for (const suiteId of gate.suiteIds ?? []) {
      for (const entry of manifest.cases) {
        if (entry.suiteId === suiteId) references.add(`${entry.suiteId}/${entry.caseId}`)
      }
    }
    if (references.size === 0) throw new Error(`Offline Eval hard gate ${gate.id} selects no cases.`)
    for (const reference of references) {
      if (!seen.has(reference)) throw new Error(`Offline Eval hard gate ${gate.id} references an unselected case: ${reference}`)
    }
  }
  return { counts, total: seen.size }
}

function buildManifestSelection(suites, manifest, manifestHash) {
  const available = suiteCaseMap(suites)
  const validation = validateBaselineManifest(manifest, suites)
  const cases = manifest.cases.map((entry) => {
    const selected = available.get(`${entry.suiteId}/${entry.caseId}`)
    return {
      suiteId: selected.suite.id,
      suiteName: selected.suite.name,
      category: entry.category,
      ...selected.item,
    }
  })
  const byCategory = Object.fromEntries(CATEGORIES.map((category) => {
    const selected = cases.filter((item) => item.category === category)
    return [category, {
      total: selected.length,
      passed: selected.filter((item) => item.passed).length,
      failed: selected.filter((item) => !item.passed).length,
    }]
  }))
  return {
    manifest: {
      id: manifest.id,
      version: manifest.version,
      schemaVersion: manifest.schemaVersion,
      hash: manifestHash,
      scorerVersion: manifest.scorerVersion,
      runPolicy: manifest.runPolicy,
      expectedCaseCounts: manifest.expectedCaseCounts,
      coverage: manifest.coverage ?? null,
    },
    summary: {
      suites: new Set(cases.map((item) => item.suiteId)).size,
      total: validation.total,
      passed: cases.filter((item) => item.passed).length,
      failed: cases.filter((item) => !item.passed).length,
      byCategory,
    },
    cases,
  }
}

export function evaluateHardGates(manifest, cases) {
  const byReference = new Map(cases.map((item) => [`${item.suiteId}/${item.id}`, item]))
  const results = (manifest.hardGates ?? []).map((gate) => {
    const references = new Set(gate.caseRefs ?? [])
    for (const suiteId of gate.suiteIds ?? []) {
      for (const item of cases) {
        if (item.suiteId === suiteId) references.add(`${item.suiteId}/${item.id}`)
      }
    }
    const failedCases = [...references].filter((reference) => !byReference.get(reference)?.passed)
    const maxFailures = Number(gate.maxFailures ?? 0)
    return {
      id: gate.id,
      kind: gate.kind ?? 'quality',
      passed: failedCases.length <= maxFailures,
      maxFailures,
      total: references.size,
      failed: failedCases.length,
      failedCases,
    }
  })
  return {
    total: results.length,
    passed: results.filter((gate) => gate.passed).length,
    failed: results.filter((gate) => !gate.passed).length,
    results,
  }
}

function defaultEnvironment() {
  const resolved = Intl.DateTimeFormat().resolvedOptions()
  return {
    node: process.version,
    platform: process.platform,
    arch: process.arch,
    timezone: resolved.timeZone ?? null,
    locale: resolved.locale ?? null,
  }
}

export async function runOfflineEvals({ manifest = 'full', repetitions, runMetadata = {} } = {}) {
  const selectedManifest = resolveEvaluationManifest(manifest)
  const allSuites = [
    evaluateModelMatrix(),
    evaluateToolContract(),
    evaluateCodingAgentContract(),
    evaluateInjectionBoundary(),
    evaluateIntentRegressions(),
    evaluateRecoveryContract(),
    evaluateRuntimeToolCatalog(),
    evaluatePermissionFailureContract(),
    evaluateGraphReviewAcceptanceContract(),
  ]
  const manifestHash = hashJson(selectedManifest)
  const datasetHash = hashJson({
    modelMatrix,
    toolContract,
    codingAgentContract,
    injectionBoundary,
    intentRegressions,
    recoveryContract,
    permissionFailureContract,
    graphReviewAcceptanceContract,
  })
  const toolCatalogHash = hashJson(RUNTIME_TOOL_CATALOG
    .map(({ name, category, execution, approval }) => ({ name, category, execution, approval }))
    .sort((left, right) => left.name.localeCompare(right.name)))
  const promptComposition = composeRuntimePrompt({
    systemPrompt: 'You are Fox.',
    context: { projectRoot: 'D:/authorized-project', workSnapshot: {} },
    turn: { date: '2026-08-06T00:00:00.000Z' },
    cache: { modelId: 'deterministic-offline', toolCatalogHash, policy: 'read_only' },
  })
  const stablePromptHash = promptComposition.stablePromptHash
  const promptRegistry = promptComposition.diagnostics.promptRegistry
  const contextSchemaHash = promptComposition.diagnostics.contextSchemaHash
  const environment = { ...defaultEnvironment(), ...(runMetadata.environment ?? {}) }
  const code = {
    commit: runMetadata.code?.commit ?? process.env.FOX_EVAL_COMMIT ?? null,
    dirty: runMetadata.code?.dirty ?? null,
  }
  const effectiveRepetitions = repetitions ?? selectedManifest.runPolicy.repetitions
  if (!Number.isInteger(effectiveRepetitions) || effectiveRepetitions < 1) {
    throw new Error('Offline Eval repetitions must be a positive integer.')
  }
  const model = {
    mode: 'deterministic-offline',
    provider: null,
    id: null,
    parameters: null,
    repetitions: effectiveRepetitions,
  }
  const baseline = buildManifestSelection(allSuites, selectedManifest, manifestHash)
  const selectedReferences = new Set(baseline.cases.map((item) => `${item.suiteId}/${item.id}`))
  const suites = allSuites
    .map((suite) => ({
      ...suite,
      cases: suite.cases.filter((item) => selectedReferences.has(`${suite.id}/${item.id}`)),
    }))
    .filter((suite) => suite.cases.length > 0)
  const cases = suites.flatMap((suite) => suite.cases)
  const hardGates = evaluateHardGates(selectedManifest, baseline.cases)
  const report = {
    kind: 'fox-offline-agent-eval',
    schemaVersion: 4,
    datasetVersion: DATASET_VERSION,
    generatedAt: new Date().toISOString(),
    metadata: {
      evaluatorVersion: EVALUATOR_VERSION,
      scorerVersion: selectedManifest.scorerVersion,
      datasetHash,
      manifestHash,
      stablePromptHash,
      promptDefinitionId: promptRegistry.definitionId,
      promptVersion: promptRegistry.version,
      promptContentHash: promptRegistry.contentHash,
      contextSchemaHash,
      promptCacheIdentity: promptComposition.diagnostics.cacheIdentity,
      promptCacheDiagnostics: promptComposition.diagnostics.cache,
      toolCatalogHash,
      code,
      model,
      modelConfigHash: hashJson(model),
      environment,
      environmentHash: hashJson(environment),
    },
    summary: {
      suites: suites.length,
      total: cases.length,
      passed: cases.filter((item) => item.passed).length,
      failed: cases.filter((item) => !item.passed).length,
    },
    baseline,
    hardGates,
    suites,
  }
  report.resultHash = hashJson({
    kind: report.kind,
    schemaVersion: report.schemaVersion,
    datasetVersion: report.datasetVersion,
    identity: {
      evaluatorVersion: report.metadata.evaluatorVersion,
      scorerVersion: report.metadata.scorerVersion,
      datasetHash,
      manifestHash,
      stablePromptHash,
      promptDefinitionId: promptRegistry.definitionId,
      promptVersion: promptRegistry.version,
      promptContentHash: promptRegistry.contentHash,
      contextSchemaHash,
      toolCatalogHash,
      modelConfigHash: report.metadata.modelConfigHash,
    },
    summary: report.summary,
    baseline,
    hardGates,
    suites,
  })
  return report
}

function reportIdentity(report, key, fallback = null) {
  if (key === 'foxCommit') return report?.metadata?.code?.commit ?? fallback
  if (key === 'dirtyWorktree') return report?.metadata?.code?.dirty ?? fallback
  return report?.metadata?.[key] ?? fallback
}

function baselineCaseMap(report) {
  return new Map((report?.baseline?.cases ?? []).map((item) => [`${item.suiteId}/${item.id}`, item]))
}

export function compareEvaluationReports(current, baseline) {
  const baselineSuites = new Map((baseline?.suites ?? []).map((suite) => [suite.id ?? suite.name, suite]))
  const suites = current.suites.map((suite) => {
    const before = baselineSuites.get(suite.id ?? suite.name)
    const passed = suite.cases.filter((item) => item.passed).length
    const previousPassed = before?.cases?.filter((item) => item.passed).length ?? null
    return {
      id: suite.id ?? null,
      name: suite.name,
      passed,
      total: suite.cases.length,
      deltaPassed: previousPassed === null ? null : passed - previousPassed,
    }
  })

  const currentCases = baselineCaseMap(current)
  const previousCases = baselineCaseMap(baseline)
  const caseRegressions = []
  const caseImprovements = []
  const missingCases = []
  const addedCases = []
  for (const [reference, before] of previousCases) {
    const after = currentCases.get(reference)
    if (!after) missingCases.push(reference)
    else if (before.passed && !after.passed) caseRegressions.push(reference)
    else if (!before.passed && after.passed) caseImprovements.push(reference)
  }
  for (const reference of currentCases.keys()) {
    if (!previousCases.has(reference)) addedCases.push(reference)
  }

  const categoryDeltas = Object.fromEntries(CATEGORIES.map((category) => {
    const currentCategory = current?.baseline?.summary?.byCategory?.[category]
    const previousCategory = baseline?.baseline?.summary?.byCategory?.[category]
    return [category, {
      passed: currentCategory?.passed ?? null,
      total: currentCategory?.total ?? null,
      deltaPassed: currentCategory && previousCategory ? currentCategory.passed - previousCategory.passed : null,
    }]
  }))

  const identityFields = [
    'evaluatorVersion',
    'datasetHash',
    'manifestHash',
    'scorerVersion',
    'stablePromptHash',
    'promptDefinitionId',
    'promptVersion',
    'promptContentHash',
    'contextSchemaHash',
    'toolCatalogHash',
    'modelConfigHash',
    'foxCommit',
    'dirtyWorktree',
    'environmentHash',
  ]
  const identityDifferences = identityFields.flatMap((field) => {
    const before = reportIdentity(baseline, field)
    const after = reportIdentity(current, field)
    return before !== after ? [{ field, baseline: before, current: after }] : []
  })
  const hardIdentityFields = new Set([
    'evaluatorVersion',
    'datasetHash',
    'manifestHash',
    'scorerVersion',
    'contextSchemaHash',
    'modelConfigHash',
    'toolCatalogHash',
    'environmentHash',
  ])
  const incompatibilities = identityDifferences.filter((item) => hardIdentityFields.has(item.field))
  for (const field of hardIdentityFields) {
    const before = reportIdentity(baseline, field)
    const after = reportIdentity(current, field)
    const complete = typeof before === 'string'
      && before.trim().length > 0
      && typeof after === 'string'
      && after.trim().length > 0
    if (!complete && !incompatibilities.some((item) => item.field === field)) {
      incompatibilities.push({
        field,
        baseline: before,
        current: after,
        reason: 'both reports must declare a non-empty hard identity',
      })
    }
  }
  const baselineCommit = reportIdentity(baseline, 'foxCommit')
  const currentCommit = reportIdentity(current, 'foxCommit')
  if (typeof baselineCommit !== 'string'
    || baselineCommit.trim().length === 0
    || typeof currentCommit !== 'string'
    || currentCommit.trim().length === 0
    || baselineCommit !== currentCommit) {
    incompatibilities.push({
      field: 'foxCommit',
      baseline: baselineCommit,
      current: currentCommit,
      reason: 'both reports must declare the same non-empty Fox commit',
    })
  }
  const baselineDirty = reportIdentity(baseline, 'dirtyWorktree')
  const currentDirty = reportIdentity(current, 'dirtyWorktree')
  if (baselineDirty !== false || currentDirty !== false) {
    incompatibilities.push({
      field: 'dirtyWorktree',
      baseline: baselineDirty,
      current: currentDirty,
      reason: 'release comparison requires both reports to declare dirtyWorktree=false',
    })
  }
  if (baseline?.datasetVersion !== current?.datasetVersion && !incompatibilities.some((item) => item.field === 'datasetHash')) {
    incompatibilities.push({ field: 'datasetVersion', baseline: baseline?.datasetVersion ?? null, current: current?.datasetVersion ?? null })
  }
  const previousGates = new Map((baseline?.hardGates?.results ?? []).map((gate) => [gate.id, gate]))
  const gateRegressions = (current?.hardGates?.results ?? [])
    .filter((gate) => previousGates.get(gate.id)?.passed && !gate.passed)
    .map((gate) => gate.id)

  return {
    baselineGeneratedAt: baseline?.generatedAt ?? null,
    baselineResultHash: baseline?.resultHash ?? null,
    currentResultHash: current?.resultHash ?? null,
    comparable: incompatibilities.length === 0,
    incompatibilities,
    identityDifferences,
    totalDelta: current.summary.passed - Number(baseline?.summary?.passed ?? current.summary.passed),
    quickBaselineDelta: current.baseline && baseline?.baseline
      ? current.baseline.summary.passed - baseline.baseline.summary.passed
      : null,
    regressed: suites.filter((suite) => suite.deltaPassed !== null && suite.deltaPassed < 0).map((suite) => suite.name),
    caseRegressions,
    caseImprovements,
    missingCases,
    addedCases,
    gateRegressions,
    categories: categoryDeltas,
    suites,
  }
}

function percentile(values, ratio) {
  if (values.length === 0) return null
  const sorted = [...values].sort((left, right) => left - right)
  return sorted[Math.max(0, Math.ceil(sorted.length * ratio) - 1)]
}

function distribution(values) {
  return {
    count: values.length,
    p50: percentile(values, 0.5),
    p95: percentile(values, 0.95),
  }
}

function tokenUsage(trial) {
  const source = trial?.tokens ?? trial?.report?.usage?.tokens ?? trial?.report?.usage
  if (!source || typeof source !== 'object') return null
  const usage = {
    input: source.input ?? source.inputTokens,
    output: source.output ?? source.outputTokens,
    cacheRead: source.cacheRead ?? source.cacheReadTokens,
    cacheWrite: source.cacheWrite ?? source.cacheWriteTokens,
    total: source.total ?? source.totalTokens,
  }
  if (usage.total === undefined && Number.isFinite(usage.input) && Number.isFinite(usage.output)) {
    usage.total = usage.input + usage.output
  }
  const normalized = Object.fromEntries(Object.entries(usage).flatMap(([key, value]) => (
    Number.isFinite(value) && value >= 0 ? [[key, Number(value)]] : []
  )))
  return Object.keys(normalized).length > 0 ? normalized : null
}

export function aggregateEvaluationRuns(trials) {
  if (!Array.isArray(trials) || trials.length === 0) {
    throw new Error('Offline Eval aggregation requires at least one trial.')
  }
  const normalized = trials.map((trial, index) => ({
    id: trial?.id ?? `trial-${index + 1}`,
    success: typeof trial?.success === 'boolean'
      ? trial.success
      : Number(trial?.report?.summary?.failed ?? 1) === 0,
    errorCompletion: Boolean(
      trial?.errorCompletion
      ?? trial?.erroneousCompletion
      ?? trial?.report?.metrics?.errorCompletion
      ?? trial?.report?.metrics?.erroneousCompletion,
    ),
    latencyMs: Number.isFinite(trial?.latencyMs) && trial.latencyMs >= 0 ? Number(trial.latencyMs) : null,
    tokens: tokenUsage(trial),
  }))
  const successes = normalized.filter((trial) => trial.success).length
  const errorCompletions = normalized.filter((trial) => trial.errorCompletion).length
  const latencyValues = normalized.flatMap((trial) => trial.latencyMs === null ? [] : [trial.latencyMs])
  const tokenTrials = normalized.filter((trial) => trial.tokens !== null)
  const tokenFields = ['input', 'output', 'cacheRead', 'cacheWrite', 'total']
  const tokens = tokenTrials.length === 0 ? null : {
    runsWithData: tokenTrials.length,
    totals: Object.fromEntries(tokenFields.map((field) => [
      field,
      tokenTrials.reduce((sum, trial) => sum + Number(trial.tokens[field] ?? 0), 0),
    ])),
    perRun: Object.fromEntries(tokenFields.map((field) => [
      field,
      distribution(tokenTrials.flatMap((trial) => trial.tokens[field] === undefined ? [] : [trial.tokens[field]])),
    ])),
  }
  return {
    total: normalized.length,
    successes,
    successRate: successes / normalized.length,
    errorCompletions,
    errorCompletionRate: errorCompletions / normalized.length,
    latencyMs: distribution(latencyValues),
    tokens,
    trials: normalized.map(({ id, success, errorCompletion, latencyMs }) => ({ id, success, errorCompletion, latencyMs })),
  }
}

export function evaluationFailureReasons(report) {
  const reasons = []
  if (Number(report?.summary?.failed ?? 0) > 0) reasons.push({ code: 'case_failure', count: report.summary.failed })
  if (Number(report?.hardGates?.failed ?? 0) > 0) {
    reasons.push({
      code: 'hard_gate_failure',
      gates: report.hardGates.results.filter((gate) => !gate.passed).map((gate) => gate.id),
    })
  }
  if (Number(report?.aggregation?.errorCompletions ?? 0) > 0) {
    reasons.push({ code: 'error_completion', count: report.aggregation.errorCompletions })
  }
  if (report?.comparison?.comparable === false) {
    reasons.push({ code: 'incomparable_baseline', fields: report.comparison.incompatibilities.map((item) => item.field) })
  }
  if ((report?.comparison?.regressed?.length ?? 0) > 0) {
    reasons.push({ code: 'suite_regression', suites: report.comparison.regressed })
  }
  if ((report?.comparison?.caseRegressions?.length ?? 0) > 0) {
    reasons.push({ code: 'case_regression', cases: report.comparison.caseRegressions })
  }
  if ((report?.comparison?.gateRegressions?.length ?? 0) > 0) {
    reasons.push({ code: 'gate_regression', gates: report.comparison.gateRegressions })
  }
  if ((report?.comparison?.missingCases?.length ?? 0) > 0) {
    reasons.push({ code: 'missing_case', cases: report.comparison.missingCases })
  }
  return reasons
}
