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
  passed: report.summary.passed,
  failed: report.summary.failed,
  total: report.summary.total,
  passRate: report.summary.total ? report.summary.passed / report.summary.total : 0,
  suites: Object.fromEntries(report.suites.map((suite) => [
    suite.name,
    `${suite.cases.filter((item) => item.passed).length}/${suite.cases.length}`,
  ])),
}))
process.stdout.write(`${JSON.stringify({ kind: 'fox-offline-agent-eval-trend', reports: trend }, null, 2)}\n`)
