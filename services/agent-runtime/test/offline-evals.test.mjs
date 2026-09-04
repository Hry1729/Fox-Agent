import test from 'node:test'
import assert from 'node:assert/strict'
import {
  aggregateEvaluationRuns,
  compareEvaluationReports,
  evaluateHardGates,
  evaluationFailureReasons,
  runOfflineEvals,
} from '../evals/run-offline-evals.mjs'
import {
  applyExecutionProfileToTools,
  executionProfilePrompt,
  GRAPH_REVIEWER_DECISION_OUTPUT_SCHEMA,
  resolveExecutionProfile,
} from '../src/execution-profile.mjs'
import { createHostTools } from '../src/host-tools.mjs'
import { resolveEvaluationManifest } from '../src/offline-evaluator.mjs'
import { RUNTIME_TOOL_CATALOG } from '../src/runtime-contract.mjs'
import { resolvePromptPolicy } from '../src/prompt-registry.mjs'

function reproducibleRunMetadata() {
  return {
    code: { commit: '0123456789abcdef', dirty: false },
    environment: { node: 'test-node', platform: 'test-platform', arch: 'test-arch', timezone: 'UTC', locale: 'en' },
  }
}

test('passes the Phase 0B full manifest with complete Runtime tool coverage', async () => {
  const report = await runOfflineEvals()
  assert.equal(report.schemaVersion, 4)
  assert.equal(report.summary.suites, 9)
  assert.equal(report.summary.total, 121)
  assert.equal(report.summary.failed, 0, JSON.stringify(report.suites.filter((suite) => suite.cases.some((item) => !item.passed)), null, 2))
  assert.equal(report.metadata.evaluatorVersion, 'fox-offline-evaluator-v8')
  const injectionCases = report.suites.find((suite) => suite.id === 'injection-boundary').cases
  assert.equal(injectionCases.length, 4)
  assert.equal(injectionCases.every((item) => item.isolatedInHostWorkSnapshot && item.workSnapshotAuthorityCount === 1), true)
  assert.equal(injectionCases.some((item) => 'isolatedInWorkspaceContext' in item), false)
  assert.equal(report.baseline.manifest.id, 'phase-0b-full-baseline')
  assert.equal(report.baseline.manifest.version, '2026-08-27.3')
  assert.equal(report.baseline.summary.total, 121)
  assert.deepEqual(
    Object.fromEntries(Object.entries(report.baseline.summary.byCategory).map(([category, summary]) => [category, summary.total])),
    { simple: 10, medium: 81, complex: 25, recovery: 5 },
  )
  const runtimeToolCatalog = report.suites.find((suite) => suite.id === 'runtime-tool-catalog')
  assert.equal(runtimeToolCatalog.cases.length, 65)
  const graphNodeFinish = runtimeToolCatalog.cases.find((item) => item.id === 'graph_readonly_node_finish')
  assert.equal(graphNodeFinish.passed, true)
  assert.deepEqual(graphNodeFinish.actual, {
    name: 'graph_readonly_node_finish',
    category: 'work',
    execution: 'host',
    approval: 'none',
    uniqueName: true,
    protectedMutation: true,
    runtimeIsReadOnly: true,
  })
  const graphNodeCancel = runtimeToolCatalog.cases.find((item) => item.id === 'graph_readonly_node_cancel')
  assert.equal(graphNodeCancel.passed, true)
  assert.deepEqual(graphNodeCancel.actual, {
    name: 'graph_readonly_node_cancel',
    category: 'work',
    execution: 'host',
    approval: 'none',
    uniqueName: true,
    protectedMutation: true,
    runtimeIsReadOnly: true,
  })
  assert.equal(
    report.baseline.cases.find((item) => item.suiteId === 'runtime-tool-catalog' && item.id === 'graph_readonly_node_cancel').category,
    'medium',
  )
  for (const toolName of ['graph_readonly_node_review', 'graph_readonly_accept']) {
    const toolCase = runtimeToolCatalog.cases.find((item) => item.id === toolName)
    assert.equal(toolCase.passed, true)
    assert.deepEqual(toolCase.actual, {
      name: toolName,
      category: 'work',
      execution: 'host',
      approval: 'none',
      uniqueName: true,
      protectedMutation: true,
      runtimeIsReadOnly: true,
    })
    assert.equal(
      report.baseline.cases.find((item) => item.suiteId === 'runtime-tool-catalog' && item.id === toolName).category,
      'medium',
    )
  }
  const graphAcceptance = report.suites.find((suite) => suite.id === 'graph-review-acceptance-contract')
  assert.equal(graphAcceptance.cases.length, 3)
  assert.equal(graphAcceptance.cases.every((item) => item.passed), true, JSON.stringify(graphAcceptance.cases, null, 2))
  assert.deepEqual(graphAcceptance.cases.map((item) => item.id), [
    'node-review-independent-current-attempt',
    'separate-node-review-and-goal-acceptance',
    'final-current-plan-clean-findings-only',
  ])
  assert.deepEqual(
    graphAcceptance.cases[0].checks,
    {
      exactReviewSchema: true,
      finishBoundsReused: true,
      durableV2Only: true,
      currentAttemptFreshEvidence: true,
      independentReviewer: true,
      allowlistedToolProof: true,
      boundedReviewerProfile: true,
      exactFourKeyOutput: true,
      boundedStructuredFindings: true,
      outcomeFindingRules: true,
      fixtureRequiresStructuredFindings: true,
    },
  )
  assert.equal(report.suites.find((suite) => suite.id === 'permission-failure-contract').cases.length, 13)
  assert.deepEqual(
    { total: report.hardGates.total, passed: report.hardGates.passed, failed: report.hardGates.failed },
    { total: 6, passed: 6, failed: 0 },
  )
  assert.equal(report.metadata.datasetHash, '610af6adb505549587222e7410ac7003b46241b974174e2a96ccb01fddadf933')
  assert.equal(report.metadata.manifestHash, '7265fa9d4bd7d76fc9ffd987c7be73e175b13f222682064349a570631aff3929')
  assert.equal(report.metadata.toolCatalogHash, 'a91c163d553090b699b4852546d4cd38bb6b03a5e3302ed41c4029a382cdb6d3')
  assert.match(report.metadata.modelConfigHash, /^[a-f0-9]{64}$/)
  assert.match(report.metadata.environmentHash, /^[a-f0-9]{64}$/)
  assert.match(report.metadata.stablePromptHash, /^[a-f0-9]{16,64}$/)
  assert.equal(report.metadata.promptDefinitionId, 'fox.runtime.system')
  assert.equal(report.metadata.promptVersion, '1.0.0')
  assert.match(report.metadata.promptContentHash, /^[a-f0-9]{64}$/)
  assert.match(report.metadata.contextSchemaHash, /^[a-f0-9]{64}$/)
  assert.match(report.metadata.promptCacheIdentity.cacheKey, /^[a-f0-9]{64}$/)
  assert.equal(report.metadata.promptCacheIdentity.toolCatalogHash, report.metadata.toolCatalogHash)
  assert.equal(report.metadata.promptCacheDiagnostics.read.eligible, true)
  assert.equal(report.metadata.promptCacheDiagnostics.write.eligible, false)
  assert.equal(report.resultHash, '39e1dcd149f391a58bff2ac9445c58ec72f094038830af4a182f1de8fa885b2d')
})

test('locks Graph review and final Acceptance schemas, profile boundary, and truthful completion Prompt', () => {
  const tools = createHostTools(() => {})
  const review = tools.find((tool) => tool.name === 'graph_readonly_node_review')
  const finish = tools.find((tool) => tool.name === 'graph_readonly_node_finish')
  const accept = tools.find((tool) => tool.name === 'graph_readonly_accept')
  assert.ok(review)
  assert.ok(finish)
  assert.ok(accept)
  const reviewKeys = [
    'goalId',
    'taskId',
    'attemptId',
    'expectedTaskVersion',
    'expectedAttemptVersion',
    'criterionEvidence',
    'summary',
  ]
  assert.deepEqual(Object.keys(review.parameters.properties), reviewKeys)
  assert.deepEqual(review.parameters.required, reviewKeys)
  assert.equal(review.parameters.additionalProperties, false)
  assert.deepEqual(review.parameters, finish.parameters)
  assert.match(review.description, /running high-risk read-only Graph Attempt whose implementation Child completed/)
  assert.deepEqual(Object.keys(accept.parameters.properties), ['goalId', 'expectedGoalVersion', 'summary'])
  assert.deepEqual(accept.parameters.required, ['goalId', 'expectedGoalVersion', 'summary'])
  assert.equal(accept.parameters.additionalProperties, false)

  for (const toolName of ['graph_readonly_node_review', 'graph_readonly_accept']) {
    assert.deepEqual(
      RUNTIME_TOOL_CATALOG.find((tool) => tool.name === toolName),
      { name: toolName, category: 'work', execution: 'host', approval: 'none' },
    )
    const profileTool = { name: toolName }
    const durable = applyExecutionProfileToTools([profileTool], resolveExecutionProfile('durable_v2'))
    assert.deepEqual(durable.tools, [profileTool])
    assert.deepEqual(durable.excludedTools, [])
    for (const profileId of ['legacy', 'durable_v2_shadow', 'graph_reviewer_v1', 'graph_readonly_preview']) {
      const excluded = applyExecutionProfileToTools([profileTool], resolveExecutionProfile(profileId))
      assert.deepEqual(excluded.tools, [], profileId)
      assert.deepEqual(excluded.excludedTools.map((item) => item.name), [toolName], profileId)
    }
  }

  const prompt = executionProfilePrompt(resolveExecutionProfile('durable_v2'))
  assert.match(prompt, /Child completed is not a Reviewer pass.*Reviewer pass is not Node accepted.*Node accepted is not Goal accepted/)
  assert.match(prompt, /current Attempt exactly once.*fresh, live, Host-valid Evidence/)
  assert.match(prompt, /graph_readonly_node_review.*exact Task and Attempt versions.*criterionEvidence/)
  assert.match(prompt, /current Lead.*implementation Child.*cannot review their own work/)
  assert.match(prompt, /independent Reviewer.*allowlisted read-only ToolCall/)
  assert.match(prompt, /Reviewer output is strict and Host-checked.*pass means every frozen criterion passed.*structured findings is empty/)
  assert.match(prompt, /revise means at least one failed criterion.*bounded structured findings/)
  assert.match(prompt, /Reviewer pass still does not accept the Node.*graph_readonly_node_finish.*passed review request unchanged/)
  assert.match(prompt, /same Goal, Task, Attempt and expected versions.*exactly the same current Attempt criterionEvidence and summary/)
  assert.match(prompt, /every required Graph node is accepted.*current approved Plan.*no open review findings.*graph_readonly_accept/)
  assert.match(prompt, /Generic acceptance_submit, goal_complete, and generic Attempt finish cannot accept or complete a Graph Goal/)
  assert.match(prompt, /successful Graph tool result is not proof that the Host transition happened/)

  const reviewerProfile = resolveExecutionProfile('graph_reviewer_v1')
  assert.deepEqual(reviewerProfile.strategies, {
    completionAudit: 'strict_v2',
    validationPolicy: 'high_risk_v1',
    promptPolicy: 'graph_reviewer_v1',
  })
  assert.equal(reviewerProfile.continuation.mode, 'disabled')
  assert.deepEqual(reviewerProfile.graph, { mode: 'disabled', maxNodes: 0, maxDepth: 0, writersAllowed: false })
  assert.equal(resolvePromptPolicy({
    profileId: reviewerProfile.id,
    promptPolicy: reviewerProfile.strategies.promptPolicy,
  }).policy, 'graph_reviewer_v1')
  const reviewerTools = applyExecutionProfileToTools(RUNTIME_TOOL_CATALOG, reviewerProfile)
  assert.deepEqual(reviewerTools.tools.map(({ name }) => name), ['read', 'ls', 'find', 'grep'])
  assert.ok(reviewerTools.tools.every(({ category }) => category === 'project-read'))
  assert.ok(reviewerTools.excludedTools.some(({ name }) => name === 'continuation_propose') === false)
  for (const forbidden of [
    'graph_readonly_run',
    'graph_readonly_snapshot_get',
    'graph_readonly_node_review',
    'graph_readonly_accept',
    'acceptance_submit',
    'child_run_start',
    'approval_request',
    'write_file',
    'run_command',
  ]) {
    assert.equal(reviewerTools.tools.some(({ name }) => name === forbidden), false, forbidden)
  }
  const reviewerPrompt = executionProfilePrompt(reviewerProfile)
  assert.match(reviewerPrompt, /independent high-risk Graph Reviewer, not the Lead and not the implementation Child/)
  assert.match(reviewerPrompt, /only read, ls, find, and grep/)
  assert.match(reviewerPrompt, /fresh evidence from a real allowlisted read-only ToolCall in this Reviewer Run/)
  assert.match(reviewerPrompt, /uncertainty or open finding must fail closed/)
  const decisionSchema = GRAPH_REVIEWER_DECISION_OUTPUT_SCHEMA
  assert.equal(decisionSchema.additionalProperties, false)
  assert.deepEqual(decisionSchema.required, ['criteria', 'recommendation', 'summary', 'findings'])
  assert.deepEqual(Object.keys(decisionSchema.properties), ['criteria', 'recommendation', 'summary', 'findings'])
  assert.deepEqual(decisionSchema.properties.findings.items.required, ['criterion', 'severity', 'title', 'detail', 'toolCallIds'])
  assert.deepEqual(
    decisionSchema.properties.findings.items.properties.severity.enum,
    ['critical', 'high', 'medium', 'low', 'info'],
  )
  assert.equal(decisionSchema.properties.findings.maxItems, 16)
  assert.equal(decisionSchema.properties.findings.items.properties.title.maxLength, 200)
  assert.equal(decisionSchema.properties.findings.items.properties.detail.maxLength, 2_000)
  assert.equal(decisionSchema.properties.findings.items.properties.toolCallIds.minItems, 1)
  assert.equal(decisionSchema.properties.findings.items.properties.toolCallIds.maxItems, 16)
  assert.equal(decisionSchema.properties.findings.items.properties.toolCallIds.uniqueItems, true)
  assert.match(reviewerPrompt, /exactly these four keys.*criteria.*recommendation.*summary.*findings/)
  assert.match(reviewerPrompt, /For pass.*status passed.*findings must be empty/)
  assert.match(reviewerPrompt, /For revise.*status failed.*findings must contain 1 to 16 items/)
  assert.match(reviewerPrompt, /finding criterion must exactly equal a frozen criterion whose status is failed/)
  assert.match(reviewerPrompt, /toolCallIds must be a subset of that criterion item's toolCallIds/)
  assert.match(reviewerPrompt, /For inconclusive, findings must be empty/)
})

test('locks the medium graph node cancel schema, profile boundary, and two-phase completion contract', () => {
  const cancelTool = createHostTools(() => {}).find((tool) => tool.name === 'graph_readonly_node_cancel')
  assert.ok(cancelTool)
  const exactKeys = [
    'goalId',
    'taskId',
    'attemptId',
    'expectedTaskVersion',
    'expectedAttemptVersion',
    'reason',
  ]
  assert.deepEqual(Object.keys(cancelTool.parameters.properties), exactKeys)
  assert.deepEqual(cancelTool.parameters.required, exactKeys)
  assert.equal(cancelTool.parameters.additionalProperties, false)

  assert.deepEqual(
    RUNTIME_TOOL_CATALOG.find((tool) => tool.name === 'graph_readonly_node_cancel'),
    { name: 'graph_readonly_node_cancel', category: 'work', execution: 'host', approval: 'none' },
  )

  const profileTool = { name: 'graph_readonly_node_cancel' }
  const durable = applyExecutionProfileToTools([profileTool], resolveExecutionProfile('durable_v2'))
  assert.deepEqual(durable.tools, [profileTool])
  assert.deepEqual(durable.excludedTools, [])
  for (const profileId of ['legacy', 'durable_v2_shadow', 'graph_reviewer_v1', 'graph_readonly_preview']) {
    const excluded = applyExecutionProfileToTools([profileTool], resolveExecutionProfile(profileId))
    assert.deepEqual(excluded.tools, [], profileId)
    assert.deepEqual(excluded.excludedTools.map((item) => item.name), ['graph_readonly_node_cancel'], profileId)
  }

  const prompt = executionProfilePrompt(resolveExecutionProfile('durable_v2'))
  assert.match(prompt, /cancellation is a Host-owned two-phase intent/)
  assert.match(prompt, /successful tool result does not itself prove the Child is cancelled/)
  assert.match(prompt, /authoritative Child terminal is completed.*fresh Evidence.*graph_readonly_node_finish/)
  assert.match(prompt, /failed, cancelled, or interrupted.*Host reconciles that actual terminal/)
})

test('keeps the Phase 0A 30-case fast manifest unchanged and independently runnable', async () => {
  const report = await runOfflineEvals({ manifest: 'fast' })
  assert.equal(report.baseline.manifest.id, 'phase-0a-fast-baseline')
  assert.equal(report.baseline.manifest.version, '2026-08-26.1')
  assert.equal(report.baseline.manifest.hash, '35808d77ddbd382d34d97645fbf5f39fea16f929c3ebd30819a4b924479b0389')
  assert.equal(report.summary.total, 30)
  assert.equal(report.summary.failed, 0)
  assert.deepEqual(
    Object.fromEntries(Object.entries(report.baseline.summary.byCategory).map(([category, summary]) => [category, summary.total])),
    { simple: 10, medium: 10, complex: 5, recovery: 5 },
  )
  assert.equal(report.hardGates.total, 0)
})

test('flags suite, case, and category regressions against a versioned baseline', async () => {
  const baseline = await runOfflineEvals({ runMetadata: reproducibleRunMetadata() })
  const current = structuredClone(baseline)
  const regressed = current.suites[0].cases[0]
  regressed.passed = false
  current.summary.passed -= 1
  current.summary.failed += 1
  const baselineCase = current.baseline.cases.find((item) => item.suiteId === current.suites[0].id && item.id === regressed.id)
  baselineCase.passed = false
  current.baseline.summary.passed -= 1
  current.baseline.summary.failed += 1
  current.baseline.summary.byCategory[baselineCase.category].passed -= 1
  current.baseline.summary.byCategory[baselineCase.category].failed += 1
  const comparison = compareEvaluationReports(current, baseline)
  assert.equal(comparison.comparable, true)
  assert.equal(comparison.totalDelta, -1)
  assert.equal(comparison.quickBaselineDelta, -1)
  assert.deepEqual(comparison.regressed, [current.suites[0].name])
  assert.deepEqual(comparison.caseRegressions, [`${current.suites[0].id}/${regressed.id}`])
  assert.equal(comparison.categories[baselineCase.category].deltaPassed, -1)
})

test('enforces zero-tolerance security hard gates and exposes CLI failure reasons', async () => {
  const report = await runOfflineEvals()
  const cases = structuredClone(report.baseline.cases)
  const injection = cases.find((item) => item.suiteId === 'injection-boundary')
  injection.passed = false
  report.summary.failed = 1
  report.hardGates = evaluateHardGates(resolveEvaluationManifest('full'), cases)
  const failedGate = report.hardGates.results.find((gate) => gate.id === 'injection-boundary')
  assert.equal(failedGate.passed, false)
  assert.deepEqual(failedGate.failedCases, [`injection-boundary/${injection.id}`])
  assert.deepEqual(
    evaluationFailureReasons(report).map((reason) => reason.code),
    ['case_failure', 'hard_gate_failure'],
  )
})

test('aggregates repeated non-deterministic outcomes, latency percentiles, and optional tokens', () => {
  const aggregation = aggregateEvaluationRuns([
    { id: 'one', success: true, errorCompletion: false, latencyMs: 10, tokens: { input: 100, output: 20, total: 120 } },
    { id: 'two', success: false, errorCompletion: true, latencyMs: 100, tokens: { inputTokens: 200, outputTokens: 40, totalTokens: 240 } },
    { id: 'three', success: true, errorCompletion: false, latencyMs: 20 },
  ])
  assert.equal(aggregation.total, 3)
  assert.equal(aggregation.successes, 2)
  assert.equal(aggregation.successRate, 2 / 3)
  assert.equal(aggregation.errorCompletions, 1)
  assert.equal(aggregation.errorCompletionRate, 1 / 3)
  assert.deepEqual(aggregation.latencyMs, { count: 3, p50: 20, p95: 100 })
  assert.equal(aggregation.tokens.runsWithData, 2)
  assert.deepEqual(aggregation.tokens.totals, { input: 300, output: 60, cacheRead: 0, cacheWrite: 0, total: 360 })
  assert.deepEqual(aggregation.tokens.perRun.total, { count: 2, p50: 120, p95: 240 })
  const report = { aggregation }
  assert.deepEqual(evaluationFailureReasons(report), [{ code: 'error_completion', count: 1 }])
})

test('keeps result hashes stable across timestamps and records environment differences separately', async () => {
  const runMetadata = reproducibleRunMetadata()
  const first = await runOfflineEvals({ runMetadata })
  const second = await runOfflineEvals({ runMetadata })
  const otherEnvironment = await runOfflineEvals({
    runMetadata: {
      code: { commit: 'fedcba9876543210', dirty: true },
      environment: { ...runMetadata.environment, timezone: 'Asia/Shanghai' },
    },
  })
  assert.equal(first.resultHash, second.resultHash)
  assert.equal(first.metadata.environmentHash, second.metadata.environmentHash)
  assert.deepEqual(first.metadata.code, runMetadata.code)
  assert.equal(first.resultHash, otherEnvironment.resultHash)
  assert.notEqual(first.metadata.environmentHash, otherEnvironment.metadata.environmentHash)
  const comparison = compareEvaluationReports(otherEnvironment, first)
  assert.equal(comparison.comparable, false)
  assert.deepEqual(
    comparison.identityDifferences.map((item) => item.field),
    ['foxCommit', 'dirtyWorktree', 'environmentHash'],
  )
  assert.deepEqual(
    comparison.incompatibilities.map((item) => item.field),
    ['environmentHash', 'foxCommit', 'dirtyWorktree'],
  )
})

test('refuses to treat a different manifest or dataset as a comparable baseline', async () => {
  const baseline = await runOfflineEvals({ runMetadata: reproducibleRunMetadata() })
  const current = structuredClone(baseline)
  current.metadata.manifestHash = '0'.repeat(64)
  current.metadata.datasetHash = '1'.repeat(64)
  const comparison = compareEvaluationReports(current, baseline)
  assert.equal(comparison.comparable, false)
  assert.deepEqual(comparison.incompatibilities.map((item) => item.field), ['datasetHash', 'manifestHash'])
})

test('refuses comparison across Typed Context schema identities', async () => {
  const baseline = await runOfflineEvals({ runMetadata: reproducibleRunMetadata() })
  const current = structuredClone(baseline)
  current.metadata.contextSchemaHash = '9'.repeat(64)
  const comparison = compareEvaluationReports(current, baseline)
  assert.equal(comparison.comparable, false)
  assert.deepEqual(comparison.incompatibilities.map((item) => item.field), ['contextSchemaHash'])
})

test('requires reproducible evaluator, model, tools, environment, commit, and clean-worktree identity', async () => {
  const baseline = await runOfflineEvals({ runMetadata: reproducibleRunMetadata() })
  for (const field of ['evaluatorVersion', 'modelConfigHash', 'toolCatalogHash', 'environmentHash']) {
    const current = structuredClone(baseline)
    current.metadata[field] = field === 'evaluatorVersion' ? 'fox-offline-evaluator-v999' : 'f'.repeat(64)
    const comparison = compareEvaluationReports(current, baseline)
    assert.equal(comparison.comparable, false, field)
    assert.ok(comparison.incompatibilities.some((item) => item.field === field), field)

    const missingBaseline = structuredClone(baseline)
    const missingCurrent = structuredClone(baseline)
    delete missingBaseline.metadata[field]
    delete missingCurrent.metadata[field]
    const missingComparison = compareEvaluationReports(missingCurrent, missingBaseline)
    assert.equal(missingComparison.comparable, false, `missing ${field}`)
    assert.ok(missingComparison.incompatibilities.some((item) => item.field === field), `missing ${field}`)
  }

  for (const code of [
    { commit: '', dirty: false },
    { commit: null, dirty: false },
    { commit: 'fedcba9876543210', dirty: false },
    { commit: '0123456789abcdef', dirty: true },
    { commit: '0123456789abcdef' },
  ]) {
    const current = structuredClone(baseline)
    current.metadata.code = code
    const comparison = compareEvaluationReports(current, baseline)
    assert.equal(comparison.comparable, false, JSON.stringify(code))
    if (code.commit !== baseline.metadata.code.commit) {
      assert.ok(comparison.incompatibilities.some((item) => item.field === 'foxCommit'))
    }
    if (code.dirty !== false) {
      assert.ok(comparison.incompatibilities.some((item) => item.field === 'dirtyWorktree'))
    }
  }

  const comparable = compareEvaluationReports(structuredClone(baseline), baseline)
  assert.equal(comparable.comparable, true)
  assert.deepEqual(comparable.incompatibilities, [])
})
