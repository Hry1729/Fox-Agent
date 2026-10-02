import test from 'node:test'
import assert from 'node:assert/strict'
import { resolveExecutionProfile, supportedExecutionProfileIds } from '../src/execution-profile.mjs'
import { composeFoxPrompt } from '../src/prompt-composer.mjs'
import {
  buildPromptCacheIdentity,
  createPromptExperiment,
  developmentPromptSupportMatrix,
  evaluatePromptExperiment,
  freezePromptSupportMatrix,
  promptRegistryDefinition,
  promptVersionSnapshot,
  renderPromptPrefix,
  resolvePromptPolicy,
  supportedPromptPolicies,
  validatePromptSupportMatrix,
  validatePromptVersionSnapshot,
} from '../src/prompt-registry.mjs'
import { typedContextSchemaHash } from '../src/typed-context.mjs'
import { composeRuntimePrompt, FOX_RUNTIME_INSTRUCTIONS } from '../src/runtime-instructions.mjs'

const HASHES = Object.freeze({
  modelConfigHash: '1'.repeat(64),
  datasetHash: '2'.repeat(64),
  toolCatalogHash: '3'.repeat(64),
  contextSchemaHash: typedContextSchemaHash(),
  environmentHash: '4'.repeat(64),
})

function identity(stablePromptHash) {
  return { modelId: 'fox-test', ...HASHES, stablePromptHash }
}

function noSideEffects(overrides = {}) {
  return {
    filesystemWrites: false,
    externalNetwork: false,
    mcpActions: false,
    childRuns: false,
    billableCalls: false,
    externalMessages: false,
    productionWrites: false,
    silentDualRun: false,
    ...overrides,
  }
}

function experimentInput(overrides = {}) {
  return {
    schemaVersion: 1,
    id: 'prompt.experiment.one',
    mode: 'offline_replay',
    baseline: identity('a'.repeat(16)),
    candidate: identity('b'.repeat(16)),
    sideEffectDeclaration: noSideEffects(),
    gates: {
      minimumQuality: 0.8,
      maximumQualityRegression: 0.02,
      maximumCostIncreaseRatio: 0.1,
      maximumTokenIncreaseRatio: 0.1,
    },
    ...overrides,
  }
}

test('provides immutable PromptDefinition and PromptVersion records with tamper detection', () => {
  const definition = promptRegistryDefinition()
  const stable = promptVersionSnapshot('stable_v1')
  const reviewer = promptVersionSnapshot('graph_reviewer_v1')
  const registry = promptVersionSnapshot('registry_v1')

  assert.equal(definition.id, 'fox.runtime.system')
  assert.deepEqual(supportedPromptPolicies(), ['stable_v1', 'graph_reviewer_v1', 'registry_v1'])
  assert.equal(definition.versions.length, 3)
  assert.equal(stable.status, 'stable')
  assert.equal(reviewer.status, 'stable')
  assert.equal(registry.status, 'development')
  assert.match(stable.contentHash, /^[a-f0-9]{64}$/)
  assert.equal(Object.isFrozen(definition), true)
  assert.equal(validatePromptVersionSnapshot(stable), stable)
  assert.equal(renderPromptPrefix(stable, { runtimeInstructions: 'Runtime', modelInstructions: 'Model' }), 'Runtime\n\nModel')

  const modified = structuredClone(stable)
  modified.template = 'forged'
  assert.throws(() => validatePromptVersionSnapshot(modified), /hash_mismatch/)
  assert.throws(() => promptVersionSnapshot('constructor'), /unsupported_policy/)
  assert.throws(() => promptVersionSnapshot('future_v9'), /unsupported_policy/)
  assert.throws(() => validatePromptVersionSnapshot({ ...structuredClone(stable), future: true }), /invalid_version/)
})

test('keeps Lead profiles on stable_v1 and gives the hidden Graph Reviewer a dedicated policy', () => {
  assert.deepEqual(supportedExecutionProfileIds(), [
    'legacy',
    'durable_v2_shadow',
    'durable_v2',
    'graph_reviewer_v1',
    'graph_readonly_preview',
  ])
  for (const id of ['legacy', 'durable_v2_shadow', 'durable_v2', 'graph_readonly_preview']) {
    const profile = resolveExecutionProfile(id)
    assert.equal(profile.strategies.promptPolicy, 'stable_v1')
    assert.equal(resolvePromptPolicy({ profileId: id, promptPolicy: profile.strategies.promptPolicy }).policy, 'stable_v1')
  }
  const reviewer = resolveExecutionProfile('graph_reviewer_v1')
  assert.equal(reviewer.strategies.promptPolicy, 'graph_reviewer_v1')
  assert.equal(resolvePromptPolicy({
    profileId: reviewer.id,
    promptPolicy: reviewer.strategies.promptPolicy,
  }).policy, 'graph_reviewer_v1')
  for (const id of ['legacy', 'durable_v2_shadow', 'durable_v2', 'graph_readonly_preview']) {
    assert.throws(() => resolvePromptPolicy({
      profileId: id,
      promptPolicy: 'graph_reviewer_v1',
    }), /profile_not_supported/, id)
  }
})

test('allows registry_v1 only through an exact hashed development support matrix', () => {
  assert.throws(
    () => resolvePromptPolicy({ profileId: 'prompt_registry_preview', promptPolicy: 'registry_v1' }),
    /production_matrix_required/,
  )
  const version = promptVersionSnapshot('registry_v1')
  const matrix = developmentPromptSupportMatrix()
  assert.equal(matrix.hash, '5147240611626cbe7bf62977815323856c163139b2e56df39889e8af4438f9fb')
  assert.deepEqual(validatePromptSupportMatrix(matrix), matrix)
  assert.equal(resolvePromptPolicy({
    profileId: 'prompt_registry_preview',
    promptPolicy: 'registry_v1',
    supportMatrix: matrix,
  }), version)
  const developmentComposition = composeFoxPrompt({
    runtimeInstructions: 'Stable development contract.',
    context: {
      executionProfile: {
        id: 'prompt_registry_preview',
        strategies: { promptPolicy: 'registry_v1' },
      },
      workSnapshot: { goal: null, tasks: [], evidence: [] },
    },
    promptSupportMatrix: matrix,
  })
  assert.equal(developmentComposition.diagnostics.promptRegistry.policy, 'registry_v1')
  assert.equal(developmentComposition.diagnostics.promptRegistry.status, 'development')
  assert.ok(developmentComposition.prompt.startsWith('Stable development contract.'))
  for (const profileId of supportedExecutionProfileIds()) {
    assert.throws(() => resolvePromptPolicy({
      profileId,
      promptPolicy: 'registry_v1',
      supportMatrix: matrix,
    }), /production_profile_forbidden/, profileId)
    assert.throws(() => freezePromptSupportMatrix({
      schemaVersion: 1,
      id: 'fox.prompt.registry.development.v1',
      profiles: { [profileId]: matrix.profiles.prompt_registry_preview },
    }), /production_profile_forbidden/, profileId)
  }

  const modified = structuredClone(matrix)
  modified.profiles.prompt_registry_preview.version = '9.0.0'
  assert.throws(() => validatePromptSupportMatrix(modified), /unsupported_support_entry|hash_mismatch/)
  assert.throws(() => freezePromptSupportMatrix({
    schemaVersion: 1,
    id: 'fox.prompt.registry.development.v1',
    profiles: { constructor: matrix.profiles.prompt_registry_preview },
  }), /development_profile_only|invalid_profile_id/)
  const inheritedProfiles = Object.create({ durable_v2: matrix.profiles.prompt_registry_preview })
  inheritedProfiles.prompt_registry_preview = matrix.profiles.prompt_registry_preview
  assert.throws(() => freezePromptSupportMatrix({
    schemaVersion: 1,
    id: 'fox.prompt.registry.development.v1',
    profiles: inheritedProfiles,
  }), /support matrix profiles must use a plain object prototype/)
})

test('pins stable_v1 Runtime prefix bytes and the knowledge-routing instruction hash', () => {
  const composed = composeRuntimePrompt({ turn: { date: '2026-08-27T00:00:00.000Z' } })
  assert.equal(composed.prompt.slice(0, composed.diagnostics.stableChars), FOX_RUNTIME_INSTRUCTIONS)
  // Cursor metadata is model-visible; complete refers only to stored bytes.
  // 2026-10-01: the runtime rules gained the task-stated field-convention rule
  // (use a task's machine literal verbatim), so the pinned prefix hash moved.
  assert.equal(composed.stablePromptHash, '720249384d6a3fef')
})

test('separates stable-prefix and dynamic-tail cache identities', () => {
  const version = promptVersionSnapshot('stable_v1')
  const first = buildPromptCacheIdentity({
    promptVersion: version,
    stablePrefix: 'stable runtime rules',
    dynamicTail: 'work snapshot one',
    modelId: 'fox-model',
    toolCatalogHash: '3'.repeat(64),
  })
  const second = buildPromptCacheIdentity({
    promptVersion: version,
    stablePrefix: 'stable runtime rules',
    dynamicTail: 'work snapshot two',
    modelId: 'fox-model',
    toolCatalogHash: '3'.repeat(64),
  })
  assert.equal(first.stablePrefixHash, second.stablePrefixHash)
  assert.equal(first.cacheKey, second.cacheKey)
  assert.notEqual(first.dynamicTailHash, second.dynamicTailHash)
  assert.deepEqual(first.diagnostics.read, {
    eligible: true,
    performed: false,
    hit: null,
    reason: 'provider_not_invoked_by_composer',
  })
  assert.equal(first.diagnostics.write.eligible, true)
})

test('Composer keeps stable_v1 output compatible while exposing typed/cache diagnostics', () => {
  const base = {
    runtimeInstructions: 'Stable Runtime rules.',
    modelInstructions: 'Stable Model rules.',
    context: { model: 'fox-model' },
    turn: { date: '2026-08-27T00:00:00.000Z' },
  }
  const first = composeFoxPrompt({
    ...base,
    context: { ...base.context, workSnapshot: { goal: { id: 'goal-one' }, tasks: [], evidence: [] } },
  })
  const second = composeFoxPrompt({
    ...base,
    context: { ...base.context, workSnapshot: { goal: { id: 'goal-two' }, tasks: [], evidence: [] } },
    turn: { date: '2026-08-28T00:00:00.000Z', cwd: 'D:/other' },
  })

  assert.ok(first.prompt.startsWith('Stable Runtime rules.\n\nStable Model rules.'))
  assert.equal(first.stablePromptHash, second.stablePromptHash)
  assert.equal(first.diagnostics.cacheIdentity.stablePrefixHash, second.diagnostics.cacheIdentity.stablePrefixHash)
  assert.equal(first.diagnostics.cacheIdentity.cacheKey, second.diagnostics.cacheIdentity.cacheKey)
  assert.notEqual(first.diagnostics.cacheIdentity.dynamicTailHash, second.diagnostics.cacheIdentity.dynamicTailHash)
  assert.equal(first.diagnostics.promptRegistry.policy, 'stable_v1')
  assert.equal(first.diagnostics.contextSchemaHash, typedContextSchemaHash())
  assert.equal(first.diagnostics.fragments.filter((fragment) => fragment.kind === 'work_snapshot').length, 1)
  assert.equal(first.diagnostics.fragments.find((fragment) => fragment.kind === 'work_snapshot').trust, 'host_verified')
  assert.ok(first.diagnostics.fragments.every((fragment) => /^[a-f0-9]{64}$/.test(fragment.sourceHash)))
})

test('PromptExperiment rejects every side effect and incomparable replay identity', () => {
  for (const field of Object.keys(noSideEffects())) {
    assert.throws(
      () => createPromptExperiment(experimentInput({ sideEffectDeclaration: noSideEffects({ [field]: true }) })),
      /side_effect_forbidden/,
      field,
    )
  }
  const missingDeclaration = noSideEffects()
  delete missingDeclaration.billableCalls
  assert.throws(
    () => createPromptExperiment(experimentInput({ sideEffectDeclaration: missingDeclaration })),
    /side_effect_forbidden|invalid_side_effect_declaration/,
  )
  assert.throws(() => createPromptExperiment(experimentInput({
    candidate: { ...identity('b'.repeat(16)), environmentHash: '9'.repeat(64) },
  })), /incomparable_identity.*environmentHash/)
  assert.throws(() => createPromptExperiment(experimentInput({ mode: 'live_dual_run' })), /unsupported_mode/)
  assert.throws(() => createPromptExperiment(experimentInput({
    gates: { ...experimentInput().gates, minimumQuality: '0.8' },
  })), /invalid_gates/)
  assert.throws(() => createPromptExperiment(experimentInput({
    gates: { ...experimentInput().gates, maximumTokenIncreaseRatio: null },
  })), /invalid_gates/)
  assert.throws(() => createPromptExperiment(experimentInput({
    sideEffectDeclaration: { ...noSideEffects(), externalNetwork: null },
  })), /side_effect_forbidden/)
  assert.throws(() => createPromptExperiment(experimentInput({
    sideEffectDeclaration: { ...noSideEffects(), futureEffect: false },
  })), /invalid_side_effect_declaration/)
})

test('supports only offline replay, mock, and read-only shadow experiment contracts', () => {
  for (const mode of ['offline_replay', 'mock', 'read_only_shadow']) {
    const experiment = createPromptExperiment(experimentInput({ id: `prompt.experiment.${mode}`, mode }))
    assert.equal(experiment.mode, mode)
    assert.match(experiment.hash, /^[a-f0-9]{64}$/)
  }
})

test('safety failure forces rollback even when average quality and cost look better', () => {
  const experiment = createPromptExperiment(experimentInput())
  const report = evaluatePromptExperiment(experiment, {
    baseline: { quality: 0.9, safetyFailures: 0, cost: 10, tokens: 1_000 },
    candidate: { quality: 1, safetyFailures: 1, cost: 1, tokens: 100 },
  })
  assert.equal(report.gates.find((gate) => gate.id === 'safety_zero_tolerance').passed, false)
  assert.equal(report.rollback.required, true)
  assert.deepEqual(report.rollback.reasonCodes, ['safety_zero_tolerance'])
  assert.equal(report.rollback.recommendedAction, 'keep_baseline')
  assert.equal(report.rollback.applied, false)
  assert.equal(report.rollback.productionMutationAllowed, false)
})

test('quality and cost gates produce a deterministic recommendation without production mutation', () => {
  const experiment = createPromptExperiment(experimentInput())
  const scores = {
    baseline: { quality: 0.9, safetyFailures: 0, cost: 10, tokens: 1_000 },
    candidate: { quality: 0.91, safetyFailures: 0, cost: 10.5, tokens: 1_050 },
  }
  const first = evaluatePromptExperiment(experiment, scores)
  const second = evaluatePromptExperiment(experiment, scores)
  assert.equal(first.hash, second.hash)
  assert.equal(first.rollback.required, false)
  assert.equal(first.rollback.recommendedAction, 'candidate_eligible_for_manual_promotion')
  assert.equal(first.rollback.applied, false)

  const expensive = evaluatePromptExperiment(experiment, {
    ...scores,
    candidate: { ...scores.candidate, cost: 20 },
  })
  assert.equal(expensive.rollback.required, true)
  assert.ok(expensive.rollback.reasonCodes.includes('cost_increase'))

  const tampered = structuredClone(experiment)
  tampered.future = true
  assert.throws(() => evaluatePromptExperiment(tampered, scores), /invalid_contract/)

  for (const [field, value] of [
    ['quality', '0.9'],
    ['safetyFailures', null],
    ['safetyFailures', 0.5],
    ['cost', '10'],
    ['tokens', '1000'],
    ['tokens', 1.5],
    ['tokens', -1],
  ]) {
    assert.throws(() => evaluatePromptExperiment(experiment, {
      ...scores,
      candidate: { ...scores.candidate, [field]: value },
    }), /invalid_score/, `${field}=${String(value)}`)
  }
})
