import { execFile } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import {
  aggregateEvaluationRuns,
  compareEvaluationReports,
  evaluateHardGates,
  evaluationFailureReasons,
  runOfflineEvals,
} from '../src/offline-evaluator.mjs'

export {
  aggregateEvaluationRuns,
  compareEvaluationReports,
  evaluateHardGates,
  evaluationFailureReasons,
  runOfflineEvals,
}

function option(name) {
  const index = process.argv.indexOf(name)
  return index >= 0 ? process.argv[index + 1] : null
}

const execFileAsync = promisify(execFile)

async function gitMetadata() {
  try {
    const [{ stdout: commit }, { stdout: status }] = await Promise.all([
      execFileAsync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }),
      execFileAsync('git', ['status', '--porcelain'], { encoding: 'utf8' }),
    ])
    return { commit: commit.trim() || null, dirty: status.trim().length > 0 }
  } catch {
    return { commit: null, dirty: null }
  }
}

function positiveIntegerOption(name, fallback) {
  const value = option(name)
  if (value === null) return fallback
  const parsed = Number(value)
  if (!Number.isInteger(parsed) || parsed < 1) throw new Error(`${name} must be a positive integer.`)
  return parsed
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  const manifest = option('--manifest') ?? 'full'
  const repetitions = positiveIntegerOption('--repetitions', 1)
  const runMetadata = { code: await gitMetadata() }
  const reports = []
  const trials = []
  for (let index = 0; index < repetitions; index += 1) {
    const startedAt = performance.now()
    const repeatedReport = await runOfflineEvals({ manifest, repetitions, runMetadata })
    reports.push(repeatedReport)
    trials.push({
      id: `trial-${index + 1}`,
      report: repeatedReport,
      latencyMs: performance.now() - startedAt,
    })
  }
  const report = reports[0]
  report.aggregation = {
    requestedRepetitions: repetitions,
    distinctResultHashes: [...new Set(reports.map((item) => item.resultHash))],
    ...aggregateEvaluationRuns(trials),
  }
  const baselinePath = option('--baseline')
  if (baselinePath) {
    const baseline = JSON.parse(await readFile(resolve(baselinePath), 'utf8'))
    report.comparison = compareEvaluationReports(report, baseline)
  }
  report.failureReasons = evaluationFailureReasons(report)
  const outputPath = option('--output')
  if (outputPath) {
    const absolute = resolve(outputPath)
    await mkdir(dirname(absolute), { recursive: true })
    await writeFile(absolute, `${JSON.stringify(report, null, 2)}\n`, 'utf8')
  }
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  if (report.failureReasons.length > 0) process.exitCode = 1
}
