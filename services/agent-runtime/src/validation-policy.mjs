import { createHash } from 'node:crypto'

export const VALIDATION_POLICY_SCHEMA_VERSION = 1

export const VALIDATION_CHECK_TYPES = Object.freeze([
  'test',
  'inspection',
  'review',
  'manual',
  'other',
])

export const VALIDATION_REVIEWER_POLICIES = Object.freeze([
  'legacy',
  'host_validated',
  'independent',
])

const SNAPSHOT_FIELDS = Object.freeze([
  'schemaVersion',
  'id',
  'riskLevel',
  'requiredChecks',
  'allowedCheckTypes',
  'reviewerPolicy',
  'maxRepairAttempts',
  'completionRequiresAcceptance',
  'hash',
])

const DEFINITIONS = Object.freeze({
  legacy_v1: Object.freeze({
    id: 'legacy_v1',
    riskLevel: 'legacy',
    requiredChecks: Object.freeze([]),
    allowedCheckTypes: VALIDATION_CHECK_TYPES,
    reviewerPolicy: 'legacy',
    maxRepairAttempts: 0,
    completionRequiresAcceptance: false,
  }),
  standard_v1: Object.freeze({
    id: 'standard_v1',
    riskLevel: 'standard',
    requiredChecks: Object.freeze(['inspection']),
    allowedCheckTypes: Object.freeze(['test', 'inspection', 'review', 'manual']),
    reviewerPolicy: 'host_validated',
    maxRepairAttempts: 2,
    completionRequiresAcceptance: true,
  }),
  high_risk_v1: Object.freeze({
    id: 'high_risk_v1',
    riskLevel: 'high',
    requiredChecks: Object.freeze(['inspection', 'review']),
    allowedCheckTypes: Object.freeze(['test', 'inspection', 'review', 'manual']),
    reviewerPolicy: 'independent',
    maxRepairAttempts: 1,
    completionRequiresAcceptance: true,
  }),
})

function canonicalJson(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value)
  if (Array.isArray(value)) return `[${value.map((item) => canonicalJson(item)).join(',')}]`
  return `{${Object.keys(value)
    .sort((left, right) => left.localeCompare(right))
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`
}

function policyHash(snapshot) {
  return createHash('sha256').update(canonicalJson(snapshot)).digest('hex')
}

function deepFreeze(value) {
  if (!value || typeof value !== 'object' || Object.isFrozen(value)) return value
  for (const child of Object.values(value)) deepFreeze(child)
  return Object.freeze(value)
}

function snapshotFromDefinition(definition) {
  const snapshot = {
    schemaVersion: VALIDATION_POLICY_SCHEMA_VERSION,
    id: definition.id,
    riskLevel: definition.riskLevel,
    requiredChecks: [...definition.requiredChecks],
    allowedCheckTypes: [...definition.allowedCheckTypes],
    reviewerPolicy: definition.reviewerPolicy,
    maxRepairAttempts: definition.maxRepairAttempts,
    completionRequiresAcceptance: definition.completionRequiresAcceptance,
  }
  return deepFreeze({ ...snapshot, hash: policyHash(snapshot) })
}

const SNAPSHOTS = Object.freeze(Object.fromEntries(
  Object.entries(DEFINITIONS).map(([id, definition]) => [id, snapshotFromDefinition(definition)]),
))

function invalidPolicy(message) {
  const error = new Error(message)
  error.code = 'runtime.validation_policy.invalid_snapshot'
  return error
}

function snapshotForId(id) {
  const normalized = String(id ?? '').trim()
  if (!Object.hasOwn(SNAPSHOTS, normalized)) {
    throw invalidPolicy(`Unsupported ValidationPolicy id: ${normalized || '<empty>'}. Supported policies: ${supportedValidationPolicyIds().join(', ')}`)
  }
  return SNAPSHOTS[normalized]
}

export function supportedValidationPolicyIds() {
  return Object.keys(SNAPSHOTS)
}

export function validateValidationPolicy(candidate) {
  if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) {
    throw invalidPolicy('ValidationPolicy snapshot must be an object.')
  }
  const keys = Object.keys(candidate).sort()
  const expectedKeys = [...SNAPSHOT_FIELDS].sort()
  if (canonicalJson(keys) !== canonicalJson(expectedKeys)) {
    throw invalidPolicy(`ValidationPolicy snapshot fields must be exactly: ${SNAPSHOT_FIELDS.join(', ')}.`)
  }
  if (candidate.schemaVersion !== VALIDATION_POLICY_SCHEMA_VERSION) {
    throw invalidPolicy(`Unsupported ValidationPolicy schema version: ${String(candidate.schemaVersion)}.`)
  }
  const expected = snapshotForId(candidate.id)
  for (const field of SNAPSHOT_FIELDS.filter((item) => item !== 'hash')) {
    if (canonicalJson(candidate[field]) !== canonicalJson(expected[field])) {
      throw invalidPolicy(`ValidationPolicy ${candidate.id} does not permit a custom ${field} value.`)
    }
  }
  if (candidate.hash !== expected.hash) {
    throw invalidPolicy(`ValidationPolicy ${candidate.id} hash does not match its frozen policy definition.`)
  }
  return expected
}

export function resolveValidationPolicy(input) {
  if (input === undefined || input === null || input === '') return SNAPSHOTS.legacy_v1
  if (typeof input === 'string') return snapshotForId(input)
  if (!input || typeof input !== 'object' || Array.isArray(input)) {
    throw invalidPolicy('ValidationPolicy input must be a supported id or frozen snapshot.')
  }
  const keys = Object.keys(input)
  if (keys.length === 1 && keys[0] === 'id') return snapshotForId(input.id)
  return validateValidationPolicy(input)
}

export function freezeValidationPolicy(input) {
  return resolveValidationPolicy(input)
}
