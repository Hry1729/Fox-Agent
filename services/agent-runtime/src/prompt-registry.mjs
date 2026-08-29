import { createHash } from 'node:crypto'
import { TYPED_CONTEXT_SCHEMA_VERSION, typedContextSchemaHash } from './typed-context.mjs'

export const PROMPT_REGISTRY_SCHEMA_VERSION = 1
export const PROMPT_EXPERIMENT_SCHEMA_VERSION = 1
export const DEVELOPMENT_PROMPT_PROFILE_ID = 'prompt_registry_preview'

const HASH_PATTERN = /^[a-f0-9]{16,64}$/u
const FULL_HASH_PATTERN = /^[a-f0-9]{64}$/u
const ID_PATTERN = /^[a-z][a-z0-9_.-]{0,127}$/u
const VERSION_PATTERN = /^[1-9][0-9]*\.[0-9]+\.[0-9]+$/u
const PROMPT_TEMPLATE = '{{runtimeInstructions}}{{separator}}{{modelInstructions}}'
const COMPATIBILITY = Object.freeze({
  minRuntimeProtocol: 2,
  maxRuntimeProtocol: 2,
  contextSchemaVersion: TYPED_CONTEXT_SCHEMA_VERSION,
  renderer: 'fox_prompt_composer_v1',
})

function canonicalJson(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value)
  if (Array.isArray(value)) return `[${value.map((item) => canonicalJson(item)).join(',')}]`
  return `{${Object.keys(value)
    .filter((key) => value[key] !== undefined)
    .sort((left, right) => left.localeCompare(right))
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`
}

function sha256(value) {
  return createHash('sha256').update(String(value), 'utf8').digest('hex')
}

function hashJson(value) {
  return sha256(canonicalJson(value))
}

function deepFreeze(value) {
  if (!value || typeof value !== 'object' || Object.isFrozen(value)) return value
  for (const child of Object.values(value)) deepFreeze(child)
  return Object.freeze(value)
}

function promptVersion({ policy, version, status }) {
  const core = {
    schemaVersion: PROMPT_REGISTRY_SCHEMA_VERSION,
    definitionId: 'fox.runtime.system',
    policy,
    version,
    status,
    template: PROMPT_TEMPLATE,
    compatibility: COMPATIBILITY,
    fragmentSchema: Object.freeze({
      id: 'fox.typed_context',
      version: TYPED_CONTEXT_SCHEMA_VERSION,
      hash: typedContextSchemaHash(),
    }),
  }
  return deepFreeze({ ...core, contentHash: hashJson(core) })
}

const VERSIONS = Object.freeze({
  stable_v1: promptVersion({ policy: 'stable_v1', version: '1.0.0', status: 'stable' }),
  graph_reviewer_v1: promptVersion({ policy: 'graph_reviewer_v1', version: '1.0.0', status: 'stable' }),
  registry_v1: promptVersion({ policy: 'registry_v1', version: '1.1.0', status: 'development' }),
})

const DEFINITION = deepFreeze({
  schemaVersion: PROMPT_REGISTRY_SCHEMA_VERSION,
  id: 'fox.runtime.system',
  description: 'Fox Runtime stable system-prefix composition contract.',
  versions: Object.freeze(Object.values(VERSIONS)),
})

const VERSION_KEYS = Object.freeze([
  'schemaVersion', 'definitionId', 'policy', 'version', 'status', 'template',
  'compatibility', 'fragmentSchema', 'contentHash',
])
const SUPPORT_MATRIX_KEYS = Object.freeze(['schemaVersion', 'id', 'profiles', 'hash'])
const SIDE_EFFECT_KEYS = Object.freeze([
  'filesystemWrites',
  'externalNetwork',
  'mcpActions',
  'childRuns',
  'billableCalls',
  'externalMessages',
  'productionWrites',
  'silentDualRun',
])
const EXPERIMENT_MODES = new Set(['offline_replay', 'mock', 'read_only_shadow'])
const IDENTITY_FIELDS = Object.freeze([
  'modelId',
  'modelConfigHash',
  'datasetHash',
  'toolCatalogHash',
  'stablePromptHash',
  'contextSchemaHash',
  'environmentHash',
])
const COMPARABILITY_FIELDS = Object.freeze(IDENTITY_FIELDS.filter((field) => field !== 'stablePromptHash'))
const DEVELOPMENT_SUPPORT_MATRIX_ID = 'fox.prompt.registry.development.v1'
const PRODUCTION_PROFILE_IDS = new Set(['legacy', 'durable_v2_shadow', 'durable_v2', 'graph_reviewer_v1', 'graph_readonly_preview'])

function exactKeys(value, allowed, code, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`[${code}] ${label} must be an object.`)
  }
  const prototype = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null) {
    throw new Error(`[${code}] ${label} must use a plain object prototype.`)
  }
  const allowedKeys = new Set(allowed)
  const unknown = Object.keys(value).filter((key) => !allowedKeys.has(key))
  if (unknown.length > 0) throw new Error(`[${code}] Unsupported ${label} keys: ${unknown.join(', ')}`)
}

function validateHash(value, label, { full = false } = {}) {
  const pattern = full ? FULL_HASH_PATTERN : HASH_PATTERN
  if (typeof value !== 'string' || !pattern.test(value)) {
    throw new Error(`[prompt_registry.invalid_${label}] ${label} must be a lowercase SHA-256 hash${full ? '' : ' or stable 16-char prefix'}.`)
  }
  return value
}

function validateVersionShape(snapshot) {
  exactKeys(snapshot, VERSION_KEYS, 'prompt_registry.invalid_version', 'PromptVersion')
  if (snapshot.schemaVersion !== PROMPT_REGISTRY_SCHEMA_VERSION) {
    throw new Error(`[prompt_registry.unsupported_schema] Unsupported Prompt Registry schema version: ${String(snapshot.schemaVersion)}`)
  }
  if (!ID_PATTERN.test(String(snapshot.definitionId ?? ''))) throw new Error('[prompt_registry.invalid_definition_id] Invalid PromptDefinition id.')
  if (!VERSION_PATTERN.test(String(snapshot.version ?? ''))) throw new Error('[prompt_registry.invalid_version] Invalid PromptVersion version.')
  validateHash(snapshot.contentHash, 'content_hash', { full: true })
}

export function promptRegistryDefinition() {
  return DEFINITION
}

export function supportedPromptPolicies() {
  return Object.keys(VERSIONS)
}

export function promptVersionSnapshot(input = 'stable_v1') {
  if (typeof input !== 'string') {
    exactKeys(input, ['policy'], 'prompt_registry.invalid_version_selector', 'PromptVersion selector')
  }
  const policy = typeof input === 'string' ? input : input.policy
  if (!Object.hasOwn(VERSIONS, policy)) {
    throw new Error(`[prompt_registry.unsupported_policy] Unsupported prompt policy: ${String(policy || '<empty>')}`)
  }
  return VERSIONS[policy]
}

export function validatePromptVersionSnapshot(snapshot) {
  validateVersionShape(snapshot)
  const expected = promptVersionSnapshot(snapshot.policy)
  const inputWithoutHash = Object.fromEntries(VERSION_KEYS
    .filter((key) => key !== 'contentHash')
    .map((key) => [key, snapshot[key]]))
  const actualHash = hashJson(inputWithoutHash)
  if (actualHash !== snapshot.contentHash || snapshot.contentHash !== expected.contentHash) {
    throw new Error('[prompt_registry.hash_mismatch] PromptVersion content or compatibility metadata was modified.')
  }
  if (canonicalJson(snapshot) !== canonicalJson(expected)) {
    throw new Error('[prompt_registry.snapshot_mismatch] PromptVersion is not a frozen registered version.')
  }
  return expected
}

export function freezePromptSupportMatrix(input) {
  exactKeys(input, SUPPORT_MATRIX_KEYS.filter((key) => key !== 'hash'), 'prompt_registry.invalid_support_matrix', 'support matrix')
  if (input.schemaVersion !== PROMPT_REGISTRY_SCHEMA_VERSION) {
    throw new Error(`[prompt_registry.unsupported_support_matrix_schema] Unsupported support matrix schema: ${String(input.schemaVersion)}`)
  }
  const id = String(input.id ?? '').trim()
  if (!ID_PATTERN.test(id)) throw new Error('[prompt_registry.invalid_support_matrix_id] Invalid support matrix id.')
  if (id !== DEVELOPMENT_SUPPORT_MATRIX_ID) {
    throw new Error(`[prompt_registry.development_matrix_only] The only supported development matrix id is ${DEVELOPMENT_SUPPORT_MATRIX_ID}.`)
  }
  exactKeys(input.profiles, Object.keys(input.profiles ?? {}), 'prompt_registry.invalid_support_matrix', 'support matrix profiles')
  const profileIds = Object.keys(input.profiles)
  const productionProfile = profileIds.find((profileId) => PRODUCTION_PROFILE_IDS.has(profileId))
  if (productionProfile) {
    throw new Error(`[prompt_registry.production_profile_forbidden] Production Execution Profile ${productionProfile} cannot enable registry_v1.`)
  }
  if (profileIds.length !== 1 || profileIds[0] !== DEVELOPMENT_PROMPT_PROFILE_ID) {
    throw new Error(`[prompt_registry.development_profile_only] The matrix must contain only ${DEVELOPMENT_PROMPT_PROFILE_ID}.`)
  }
  const profiles = {}
  for (const [profileId, entry] of Object.entries(input.profiles)) {
    if (!ID_PATTERN.test(profileId) || ['__proto__', 'prototype', 'constructor', 'toString', 'hasOwnProperty'].includes(profileId)) {
      throw new Error(`[prompt_registry.invalid_profile_id] Invalid profile id: ${profileId}`)
    }
    exactKeys(entry, ['promptPolicy', 'definitionId', 'version', 'contextSchemaHash'], 'prompt_registry.invalid_support_entry', `support entry ${profileId}`)
    const version = promptVersionSnapshot(entry.promptPolicy)
    if (entry.promptPolicy !== 'registry_v1'
      || entry.definitionId !== version.definitionId
      || entry.version !== version.version
      || entry.contextSchemaHash !== typedContextSchemaHash()) {
      throw new Error(`[prompt_registry.unsupported_support_entry] ${profileId} is not an exact registry_v1 support entry.`)
    }
    profiles[profileId] = { ...entry }
  }
  const core = { schemaVersion: input.schemaVersion, id, profiles }
  return deepFreeze({ ...core, hash: hashJson(core) })
}

export const DEVELOPMENT_PROMPT_SUPPORT_MATRIX = freezePromptSupportMatrix({
  schemaVersion: PROMPT_REGISTRY_SCHEMA_VERSION,
  id: DEVELOPMENT_SUPPORT_MATRIX_ID,
  profiles: {
    [DEVELOPMENT_PROMPT_PROFILE_ID]: {
      promptPolicy: 'registry_v1',
      definitionId: VERSIONS.registry_v1.definitionId,
      version: VERSIONS.registry_v1.version,
      contextSchemaHash: typedContextSchemaHash(),
    },
  },
})

export function developmentPromptSupportMatrix() {
  return DEVELOPMENT_PROMPT_SUPPORT_MATRIX
}

export function validatePromptSupportMatrix(matrix) {
  exactKeys(matrix, SUPPORT_MATRIX_KEYS, 'prompt_registry.invalid_support_matrix', 'support matrix')
  validateHash(matrix.hash, 'support_matrix_hash', { full: true })
  const rebuilt = freezePromptSupportMatrix({
    schemaVersion: matrix.schemaVersion,
    id: matrix.id,
    profiles: matrix.profiles,
  })
  if (rebuilt.hash !== matrix.hash) throw new Error('[prompt_registry.support_matrix_hash_mismatch] Prompt support matrix was modified.')
  return rebuilt
}

export function resolvePromptPolicy(input = {}) {
  exactKeys(input, ['profileId', 'promptPolicy', 'supportMatrix'], 'prompt_registry.invalid_policy_selector', 'prompt policy selector')
  const { profileId = 'legacy', promptPolicy = 'stable_v1', supportMatrix = null } = input
  const version = promptVersionSnapshot(promptPolicy)
  if (promptPolicy === 'stable_v1') return version
  if (promptPolicy === 'graph_reviewer_v1') {
    if (profileId !== 'graph_reviewer_v1') {
      throw new Error('[prompt_registry.profile_not_supported] graph_reviewer_v1 prompt policy is restricted to the hidden graph_reviewer_v1 Execution Profile.')
    }
    return version
  }
  if (PRODUCTION_PROFILE_IDS.has(profileId)) {
    throw new Error(`[prompt_registry.production_profile_forbidden] Production Execution Profile ${profileId} cannot enable registry_v1.`)
  }
  if (profileId !== DEVELOPMENT_PROMPT_PROFILE_ID) {
    throw new Error(`[prompt_registry.development_profile_only] registry_v1 is restricted to ${DEVELOPMENT_PROMPT_PROFILE_ID}.`)
  }
  if (!supportMatrix) {
    throw new Error('[prompt_registry.production_matrix_required] registry_v1 requires an explicitly frozen support matrix.')
  }
  const matrix = validatePromptSupportMatrix(supportMatrix)
  if (matrix.hash !== DEVELOPMENT_PROMPT_SUPPORT_MATRIX.hash
    || canonicalJson(matrix) !== canonicalJson(DEVELOPMENT_PROMPT_SUPPORT_MATRIX)) {
    throw new Error('[prompt_registry.development_matrix_mismatch] registry_v1 requires the pre-frozen development support matrix.')
  }
  const entry = Object.hasOwn(matrix.profiles, profileId) ? matrix.profiles[profileId] : null
  if (!entry || entry.promptPolicy !== promptPolicy || entry.version !== version.version) {
    throw new Error(`[prompt_registry.profile_not_supported] ${profileId} is not frozen for promptPolicy=${promptPolicy}.`)
  }
  return version
}

export function renderPromptPrefix(promptVersion, { runtimeInstructions = '', modelInstructions = '' } = {}) {
  const version = validatePromptVersionSnapshot(promptVersion)
  const runtime = String(runtimeInstructions || '').trim()
  const model = String(modelInstructions || '').trim()
  const separator = runtime && model ? '\n\n' : ''
  return version.template
    .replace('{{runtimeInstructions}}', runtime)
    .replace('{{separator}}', separator)
    .replace('{{modelInstructions}}', model)
}

export function buildPromptCacheIdentity({
  promptVersion = promptVersionSnapshot('stable_v1'),
  stablePrefix = '',
  dynamicTail = '',
  modelId = null,
  toolCatalogHash = null,
  cachePolicy = 'read_write',
} = {}) {
  const version = validatePromptVersionSnapshot(promptVersion)
  if (!['read_write', 'read_only', 'disabled'].includes(cachePolicy)) {
    throw new Error(`[prompt_registry.unsupported_cache_policy] Unsupported cache policy: ${String(cachePolicy)}`)
  }
  if (toolCatalogHash !== null) validateHash(toolCatalogHash, 'tool_catalog_hash')
  const stablePrefixHash = sha256(String(stablePrefix))
  const dynamicTailHash = sha256(String(dynamicTail))
  const identity = {
    schemaVersion: PROMPT_REGISTRY_SCHEMA_VERSION,
    definitionId: version.definitionId,
    promptVersion: version.version,
    promptContentHash: version.contentHash,
    stablePrefixHash,
    contextSchemaHash: version.fragmentSchema.hash,
    modelId: modelId === null ? null : String(modelId),
    toolCatalogHash,
  }
  const cacheKey = hashJson(identity)
  const readEligible = cachePolicy !== 'disabled' && stablePrefix.length > 0
  const writeEligible = cachePolicy === 'read_write' && stablePrefix.length > 0
  return deepFreeze({
    ...identity,
    cacheKey,
    dynamicTailHash,
    cachePolicy,
    diagnostics: {
      read: { eligible: readEligible, performed: false, hit: null, reason: readEligible ? 'provider_not_invoked_by_composer' : 'cache_disabled_or_empty_prefix' },
      write: { eligible: writeEligible, performed: false, reason: writeEligible ? 'provider_not_invoked_by_composer' : 'cache_write_disabled_or_empty_prefix' },
    },
  })
}

function validateExperimentIdentity(identity, label) {
  exactKeys(identity, IDENTITY_FIELDS, 'prompt_experiment.invalid_identity', `${label} identity`)
  for (const field of IDENTITY_FIELDS) {
    const value = identity[field]
    if (field === 'modelId') {
      if (typeof value !== 'string' || value.trim().length === 0 || value.length > 200) {
        throw new Error(`[prompt_experiment.invalid_identity] ${label}.${field} must be a non-empty bounded string.`)
      }
    } else validateHash(value, `${label}_${field}`, { full: field !== 'stablePromptHash' })
  }
  return { ...identity }
}

function validateSideEffects(sideEffects) {
  exactKeys(sideEffects, SIDE_EFFECT_KEYS, 'prompt_experiment.invalid_side_effect_declaration', 'sideEffectDeclaration')
  const active = SIDE_EFFECT_KEYS.filter((key) => sideEffects[key] !== false)
  if (active.length > 0) {
    throw new Error(`[prompt_experiment.side_effect_forbidden] Prompt experiments are observation-only; forbidden declarations: ${active.join(', ')}`)
  }
  return Object.fromEntries(SIDE_EFFECT_KEYS.map((key) => [key, false]))
}

export function createPromptExperiment(input) {
  exactKeys(input, [
    'schemaVersion', 'id', 'mode', 'baseline', 'candidate', 'sideEffectDeclaration', 'gates',
  ], 'prompt_experiment.invalid_contract', 'PromptExperiment')
  const schemaVersion = input.schemaVersion ?? PROMPT_EXPERIMENT_SCHEMA_VERSION
  if (schemaVersion !== PROMPT_EXPERIMENT_SCHEMA_VERSION) {
    throw new Error(`[prompt_experiment.unsupported_schema] Unsupported PromptExperiment schema: ${String(schemaVersion)}`)
  }
  const id = String(input.id ?? '').trim()
  if (!ID_PATTERN.test(id)) throw new Error('[prompt_experiment.invalid_id] Invalid PromptExperiment id.')
  const mode = String(input.mode ?? '').trim()
  if (!EXPERIMENT_MODES.has(mode)) throw new Error(`[prompt_experiment.unsupported_mode] Unsupported experiment mode: ${mode || '<empty>'}`)
  const baseline = validateExperimentIdentity(input.baseline, 'baseline')
  const candidate = validateExperimentIdentity(input.candidate, 'candidate')
  const differences = COMPARABILITY_FIELDS.flatMap((field) => baseline[field] === candidate[field]
    ? []
    : [{ field, baseline: baseline[field], candidate: candidate[field] }])
  if (differences.length > 0) {
    throw new Error(`[prompt_experiment.incomparable_identity] Baseline and candidate differ in: ${differences.map((item) => item.field).join(', ')}`)
  }
  const sideEffectDeclaration = validateSideEffects(input.sideEffectDeclaration)
  exactKeys(input.gates, ['minimumQuality', 'maximumQualityRegression', 'maximumCostIncreaseRatio', 'maximumTokenIncreaseRatio'], 'prompt_experiment.invalid_gates', 'experiment gates')
  const gates = { ...input.gates }
  if (!Object.values(gates).every((value) => typeof value === 'number' && Number.isFinite(value) && value >= 0)) {
    throw new Error('[prompt_experiment.invalid_gates] All prompt experiment gates must be finite non-negative numbers.')
  }
  const core = { schemaVersion, id, mode, baseline, candidate, sideEffectDeclaration, gates }
  return deepFreeze({ ...core, hash: hashJson(core) })
}

function scoreShape(score, label) {
  exactKeys(score, ['quality', 'safetyFailures', 'cost', 'tokens'], 'prompt_experiment.invalid_score', `${label} score`)
  const normalized = { ...score }
  if (typeof normalized.quality !== 'number'
    || !Number.isFinite(normalized.quality)
    || normalized.quality < 0
    || typeof normalized.safetyFailures !== 'number'
    || !Number.isInteger(normalized.safetyFailures)
    || normalized.safetyFailures < 0
    || typeof normalized.cost !== 'number'
    || !Number.isFinite(normalized.cost)
    || normalized.cost < 0
    || typeof normalized.tokens !== 'number'
    || !Number.isInteger(normalized.tokens)
    || normalized.tokens < 0) {
    throw new Error(`[prompt_experiment.invalid_score] Invalid ${label} score.`)
  }
  return normalized
}

function increaseRatio(candidate, baseline) {
  if (baseline === 0) return candidate === 0 ? 0 : Number.MAX_VALUE
  return Math.max(0, (candidate - baseline) / baseline)
}

export function evaluatePromptExperiment(experiment, { baseline, candidate } = {}) {
  exactKeys(experiment, [
    'schemaVersion', 'id', 'mode', 'baseline', 'candidate', 'sideEffectDeclaration', 'gates', 'hash',
  ], 'prompt_experiment.invalid_contract', 'PromptExperiment')
  const expected = createPromptExperiment({
    schemaVersion: experiment?.schemaVersion,
    id: experiment?.id,
    mode: experiment?.mode,
    baseline: experiment?.baseline,
    candidate: experiment?.candidate,
    sideEffectDeclaration: experiment?.sideEffectDeclaration,
    gates: experiment?.gates,
  })
  if (experiment?.hash !== expected.hash) throw new Error('[prompt_experiment.hash_mismatch] PromptExperiment contract was modified.')
  const before = scoreShape(baseline, 'baseline')
  const after = scoreShape(candidate, 'candidate')
  const qualityRegression = Math.max(0, before.quality - after.quality)
  const costIncreaseRatio = increaseRatio(after.cost, before.cost)
  const tokenIncreaseRatio = increaseRatio(after.tokens, before.tokens)
  const gates = [
    { id: 'safety_zero_tolerance', kind: 'safety', passed: after.safetyFailures === 0, actual: after.safetyFailures, maximum: 0 },
    { id: 'minimum_quality', kind: 'quality', passed: after.quality >= expected.gates.minimumQuality, actual: after.quality, minimum: expected.gates.minimumQuality },
    { id: 'quality_regression', kind: 'quality', passed: qualityRegression <= expected.gates.maximumQualityRegression, actual: qualityRegression, maximum: expected.gates.maximumQualityRegression },
    { id: 'cost_increase', kind: 'cost', passed: costIncreaseRatio <= expected.gates.maximumCostIncreaseRatio, actual: costIncreaseRatio, maximum: expected.gates.maximumCostIncreaseRatio },
    { id: 'token_increase', kind: 'cost', passed: tokenIncreaseRatio <= expected.gates.maximumTokenIncreaseRatio, actual: tokenIncreaseRatio, maximum: expected.gates.maximumTokenIncreaseRatio },
  ]
  const failed = gates.filter((gate) => !gate.passed)
  const rollback = failed.length > 0
  const reportCore = {
    kind: 'fox-prompt-experiment-report',
    schemaVersion: PROMPT_EXPERIMENT_SCHEMA_VERSION,
    experimentId: expected.id,
    experimentHash: expected.hash,
    mode: expected.mode,
    identity: {
      comparable: true,
      baselineStablePromptHash: expected.baseline.stablePromptHash,
      candidateStablePromptHash: expected.candidate.stablePromptHash,
      fixed: Object.fromEntries(COMPARABILITY_FIELDS.map((field) => [field, expected.baseline[field]])),
    },
    scores: { baseline: before, candidate: after },
    gates,
    rollback: {
      required: rollback,
      reasonCodes: failed.map((gate) => gate.id),
      recommendedAction: rollback ? 'keep_baseline' : 'candidate_eligible_for_manual_promotion',
      applied: false,
      productionMutationAllowed: false,
    },
  }
  return deepFreeze({ ...reportCore, hash: hashJson(reportCore) })
}
