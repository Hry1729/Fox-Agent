import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { compareEvaluationReports, runOfflineEvals } from '../src/offline-evaluator.mjs'

export { compareEvaluationReports, runOfflineEvals }

function option(name) {
  const index = process.argv.indexOf(name)
  return index >= 0 ? process.argv[index + 1] : null
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  const report = await runOfflineEvals()
  const baselinePath = option('--baseline')
  if (baselinePath) {
    const baseline = JSON.parse(await readFile(resolve(baselinePath), 'utf8'))
    report.comparison = compareEvaluationReports(report, baseline)
  }
  const outputPath = option('--output')
  if (outputPath) {
    const absolute = resolve(outputPath)
    await mkdir(dirname(absolute), { recursive: true })
    await writeFile(absolute, `${JSON.stringify(report, null, 2)}\n`, 'utf8')
  }
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  if (report.summary.failed > 0 || report.comparison?.regressed?.length > 0) process.exitCode = 1
}
