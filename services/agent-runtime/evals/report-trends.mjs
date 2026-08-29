import { readdir, readFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const directory = resolve(process.argv[2] ?? 'services/agent-runtime/evals/results')
const reports = []
for (const entry of await readdir(directory, { withFileTypes: true }).catch(() => [])) {
  if (!entry.isFile() || !entry.name.endsWith('.json')) continue
  const report = JSON.parse(await readFile(resolve(directory, entry.name), 'utf8'))
  if (report.kind !== 'fox-offline-agent-eval') continue
  reports.push({ file: entry.name, ...report })
}
reports.sort((left, right) => String(left.generatedAt).localeCompare(String(right.generatedAt)))
const trend = reports.map((report) => ({
  file: report.file,
  generatedAt: report.generatedAt,
  datasetVersion: report.datasetVersion ?? null,
  resultHash: report.resultHash ?? null,
  manifest: report.baseline?.manifest
    ? {
        id: report.baseline.manifest.id,
        version: report.baseline.manifest.version,
        hash: report.baseline.manifest.hash,
      }
    : null,
  code: report.metadata?.code ?? null,
  environmentHash: report.metadata?.environmentHash ?? null,
  hardGates: report.hardGates
    ? { passed: report.hardGates.passed, failed: report.hardGates.failed, total: report.hardGates.total }
    : null,
  aggregation: report.aggregation
    ? {
        total: report.aggregation.total,
        successRate: report.aggregation.successRate,
        errorCompletionRate: report.aggregation.errorCompletionRate,
        latencyMs: report.aggregation.latencyMs,
        tokens: report.aggregation.tokens,
      }
    : null,
  passed: report.summary.passed,
  failed: report.summary.failed,
  total: report.summary.total,
  passRate: report.summary.total ? report.summary.passed / report.summary.total : 0,
  manifestSelection: report.baseline?.summary
    ? {
        passed: report.baseline.summary.passed,
        failed: report.baseline.summary.failed,
        total: report.baseline.summary.total,
        categories: report.baseline.summary.byCategory,
      }
    : null,
  quickBaseline: report.baseline?.manifest?.id === 'phase-0a-fast-baseline'
    ? report.baseline.summary
    : null,
  suites: Object.fromEntries(report.suites.map((suite) => [
    suite.id ?? suite.name,
    `${suite.cases.filter((item) => item.passed).length}/${suite.cases.length}`,
  ])),
}))
process.stdout.write(`${JSON.stringify({ kind: 'fox-offline-agent-eval-trend', reports: trend }, null, 2)}\n`)
