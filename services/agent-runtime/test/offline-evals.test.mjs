import test from 'node:test'
import assert from 'node:assert/strict'
import { compareEvaluationReports, runOfflineEvals } from '../evals/run-offline-evals.mjs'

test('passes the checked-in offline Agent adaptation suites', async () => {
  const report = await runOfflineEvals()
  assert.equal(report.schemaVersion, 2)
  assert.equal(report.summary.suites, 5)
  assert.ok(report.summary.total >= 35)
  assert.equal(report.summary.failed, 0, JSON.stringify(report.suites.filter((suite) => suite.cases.some((item) => !item.passed)), null, 2))
  assert.ok(report.suites.some((suite) => suite.source === 'SWE-bench'))
})

test('flags a suite regression against a versioned baseline', async () => {
  const baseline = await runOfflineEvals()
  const current = structuredClone(baseline)
  current.suites[0].cases[0].passed = false
  current.summary.passed -= 1
  current.summary.failed += 1
  const comparison = compareEvaluationReports(current, baseline)
  assert.equal(comparison.totalDelta, -1)
  assert.deepEqual(comparison.regressed, [current.suites[0].name])
})
