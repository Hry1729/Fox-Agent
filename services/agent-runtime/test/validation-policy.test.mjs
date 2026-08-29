import test from 'node:test'
import assert from 'node:assert/strict'
import {
  VALIDATION_CHECK_TYPES,
  VALIDATION_POLICY_SCHEMA_VERSION,
  freezeValidationPolicy,
  resolveValidationPolicy,
  supportedValidationPolicyIds,
  validateValidationPolicy,
} from '../src/validation-policy.mjs'

test('publishes only the three supported versioned ValidationPolicy snapshots', () => {
  assert.equal(VALIDATION_POLICY_SCHEMA_VERSION, 1)
  assert.deepEqual(supportedValidationPolicyIds(), ['legacy_v1', 'standard_v1', 'high_risk_v1'])

  const legacy = freezeValidationPolicy('legacy_v1')
  const standard = freezeValidationPolicy('standard_v1')
  const highRisk = freezeValidationPolicy('high_risk_v1')

  assert.deepEqual(legacy, {
    schemaVersion: 1,
    id: 'legacy_v1',
    riskLevel: 'legacy',
    requiredChecks: [],
    allowedCheckTypes: [...VALIDATION_CHECK_TYPES],
    reviewerPolicy: 'legacy',
    maxRepairAttempts: 0,
    completionRequiresAcceptance: false,
    hash: legacy.hash,
  })
  assert.deepEqual(standard.requiredChecks, ['inspection'])
  assert.equal(standard.reviewerPolicy, 'host_validated')
  assert.equal(standard.maxRepairAttempts, 2)
  assert.equal(standard.completionRequiresAcceptance, true)
  assert.deepEqual(highRisk.requiredChecks, ['inspection', 'review'])
  assert.equal(highRisk.reviewerPolicy, 'independent')
  assert.equal(highRisk.maxRepairAttempts, 1)
  assert.equal(highRisk.completionRequiresAcceptance, true)
  for (const snapshot of [legacy, standard, highRisk]) {
    assert.match(snapshot.hash, /^[a-f0-9]{64}$/)
    assert.ok(Object.isFrozen(snapshot))
    assert.ok(Object.isFrozen(snapshot.requiredChecks))
    assert.ok(Object.isFrozen(snapshot.allowedCheckTypes))
  }
})

test('reads missing legacy policy as legacy_v1 without adding validation requirements', () => {
  const legacy = resolveValidationPolicy()
  assert.strictEqual(resolveValidationPolicy(null), legacy)
  assert.strictEqual(resolveValidationPolicy(''), legacy)
  assert.equal(legacy.id, 'legacy_v1')
  assert.deepEqual(legacy.requiredChecks, [])
  assert.equal(legacy.maxRepairAttempts, 0)
  assert.equal(legacy.completionRequiresAcceptance, false)
})

test('keeps policy hashes stable and validates a persisted JSON snapshot', () => {
  const expectedHashes = {
    legacy_v1: '4129f5db32d88d7070c59ed2351aab9c6ad59c8cc380451901bb10962c5bee92',
    standard_v1: 'bcef9ad7087b2872a0152f493cfba131a4283a3170dc8fe5824d108d19e3d04d',
    high_risk_v1: 'bb1258169c5d923dfd5fd1980dd52150ab96e159bd01d07ff0d04c48998dc5ea',
  }
  for (const id of supportedValidationPolicyIds()) {
    const first = freezeValidationPolicy(id)
    const second = freezeValidationPolicy({ id })
    const persisted = JSON.parse(JSON.stringify(first))
    assert.equal(first.hash, expectedHashes[id])
    assert.strictEqual(first, second)
    assert.strictEqual(validateValidationPolicy(persisted), first)
    assert.strictEqual(resolveValidationPolicy(persisted), first)
  }
})

test('fails closed for unknown ids, versions, fields, and hashes', () => {
  const standard = JSON.parse(JSON.stringify(freezeValidationPolicy('standard_v1')))
  assert.throws(() => resolveValidationPolicy('custom_v1'), /Unsupported ValidationPolicy id/)
  for (const inheritedKey of ['constructor', 'toString', '__proto__', 'hasOwnProperty']) {
    assert.throws(
      () => resolveValidationPolicy(inheritedKey),
      (error) => error?.code === 'runtime.validation_policy.invalid_snapshot'
        && /Unsupported ValidationPolicy id/.test(error.message),
    )
  }
  assert.throws(() => resolveValidationPolicy({}), /snapshot fields must be exactly/)
  assert.throws(
    () => validateValidationPolicy({ ...standard, schemaVersion: 2 }),
    /Unsupported ValidationPolicy schema version/,
  )
  assert.throws(
    () => validateValidationPolicy({ ...standard, extra: true }),
    /snapshot fields must be exactly/,
  )
  assert.throws(
    () => validateValidationPolicy({ ...standard, hash: '0'.repeat(64) }),
    /hash does not match/,
  )
})

test('rejects cross-policy mixes and arbitrary check or repair configuration', () => {
  const standard = JSON.parse(JSON.stringify(freezeValidationPolicy('standard_v1')))
  const highRisk = freezeValidationPolicy('high_risk_v1')
  assert.throws(
    () => validateValidationPolicy({
      ...standard,
      requiredChecks: [...highRisk.requiredChecks],
      reviewerPolicy: highRisk.reviewerPolicy,
      hash: highRisk.hash,
    }),
    /does not permit a custom requiredChecks value/,
  )
  assert.throws(
    () => validateValidationPolicy({ ...standard, allowedCheckTypes: [...standard.allowedCheckTypes, 'shell'] }),
    /does not permit a custom allowedCheckTypes value/,
  )
  assert.throws(
    () => validateValidationPolicy({ ...standard, maxRepairAttempts: 99 }),
    /does not permit a custom maxRepairAttempts value/,
  )
  assert.throws(
    () => resolveValidationPolicy({ id: 'standard_v1', reviewerPolicy: 'independent' }),
    /snapshot fields must be exactly/,
  )
})
