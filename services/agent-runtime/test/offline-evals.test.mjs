import test from 'node:test'
import assert from 'node:assert/strict'
import { runOfflineEvals } from '../evals/run-offline-evals.mjs'

test('passes the checked-in offline Agent adaptation suites', async () => {
  const report = await runOfflineEvals()
  assert.equal(report.summary.suites, 4)
  assert.ok(report.summary.total >= 25)
  assert.equal(report.summary.failed, 0, JSON.stringify(report.suites.filter((suite) => suite.cases.some((item) => !item.passed)), null, 2))
})
