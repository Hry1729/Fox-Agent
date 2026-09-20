// 运行器（harness-v1.2）。
// node evals/harness-v1/run-harness.mjs [--product-root X] [--output DIR] [--only a,b]
//   [--mode baseline|acceptance] [--evidence-max-age-ms N]
//
// 两种运行方式（O-REVIEW-03 门禁 3）：
//   baseline  —— 记录基线。允许如实记录产品失败（修复前证据），退出码 0，
//                用于把"修复前失败"与"修复后通过"分开保留。旧失败原件永不覆盖。
//   acceptance—— 验收门禁。任一已执行任务契约失败即非零退出（1）；
//                没有任何已执行任务则退出 3（不构成验收）。默认模式。
//Cargo 测试退出码本身不能掩盖 checks 内失败：rust-host 结论只取自 evidence 的 checks，
// 与 cargo 是否 exit 0 无关。
import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import os from 'node:os'
import { HARNESS_VERSION, CONTRACT_VERSION, BASE_PRODUCT_SHA, TASKS, taskById } from './tasks.mjs'
import { CHECKERS } from './checkers.mjs'
import { buildFixture, loadProductExecutor } from './fixtures.mjs'
import {
  MEASURED_PRODUCT_FILES,
  executionStampOf,
  hashHarnessDir,
  productFileDigests,
  validateEvidence,
} from './evidence-identity.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const defaultProductRoot = resolve(here, '..', '..', '..', '..')

// O-REVIEW-02 修复 1：唯一 run 目录。不复用固定临时 fixture 目录，避免跨运行残留
// 与不同 product-root 之间的 fixture 互相污染。每次运行一个独立子目录。
function uniqueRunRoot(base) {
  const stamp = new Date().toISOString().replace(/[:.]/g, '-')
  const token = Math.random().toString(36).slice(2, 10)
  return join(base, `run-${stamp}-${token}`)
}

function option(name, fallback) {
  const index = process.argv.indexOf(name)
  return index >= 0 ? process.argv[index + 1] : fallback
}

function now() {
  return new Date().toISOString()
}

// 优先用子进程；sandbox 禁止管道 stdio 时回退到纯文件读取（不 spawn）。
async function gitMetadata(productRoot) {
  try {
    const { execFile } = await import('node:child_process')
    const { promisify } = await import('node:util')
    const exec = promisify(execFile)
    const [{ stdout: commit }, { stdout: branch }, { stdout: status }] = await Promise.all([
      exec('git', ['-C', productRoot, 'rev-parse', 'HEAD']),
      exec('git', ['-C', productRoot, 'branch', '--show-current']),
      exec('git', ['-C', productRoot, 'status', '--porcelain']),
    ])
    return { commit: commit.trim() || null, branch: branch.trim() || null, dirty: status.trim().length > 0 }
  } catch (error) {
    const fallback = await gitMetadataViaFiles(productRoot)
    return { ...fallback, gitSpawnError: String(error.message ?? error) }
  }
}

function readText(path) {
  return import('node:fs/promises').then((fs) => fs.readFile(path, 'utf8').then((t) => t.trim()).catch(() => null))
}

async function gitMetadataViaFiles(productRoot) {
  const { join } = await import('node:path')
  const dotGit = join(productRoot, '.git')
  const gitdirLine = await readText(dotGit)
  let gitdir = null
  if (gitdirLine && gitdirLine.startsWith('gitdir:')) {
    gitdir = gitdirLine.slice('gitdir:'.length).trim()
    if (!gitdir.includes(':') && !gitdir.startsWith('/')) gitdir = join(productRoot, gitdir)
  } else if (await stat(dotGit).then((entry) => entry.isDirectory()).catch(() => false)) {
    gitdir = dotGit
  }
  if (!gitdir) return { commit: null, branch: null, dirty: null }
  const headRef = await readText(join(gitdir, 'HEAD'))
  if (!headRef) return { commit: null, branch: null, dirty: null }
  let branch = null
  let commit = null
  if (headRef.startsWith('ref:')) {
    branch = headRef.slice('ref:'.length).trim().replace(/^refs\/heads\//, '')
    const commondirLine = await readText(join(gitdir, 'commondir'))
    const commondir = commondirLine ? (commondirLine.includes(':') || commondirLine.startsWith('/') ? commondirLine : join(gitdir, commondirLine)) : gitdir
    commit = await readText(join(commondir, 'refs', 'heads', branch))
    if (!commit) {
      const packed = await readText(join(commondir, 'packed-refs'))
      if (packed) {
        const line = packed.split(/\r?\n/).find((row) => row.endsWith(`refs/heads/${branch}`))
        commit = line ? line.split(/\s+/)[0] : null
      }
    }
  } else {
    commit = headRef
  }
  return { commit, branch, dirty: null, dirtyNote: 'worktree 状态需 git 子进程；sandbox 下不可用，省略' }
}

// O-REVIEW-02 修复 6 / O-REVIEW-03 门禁 1：rust-host 任务的 evidence 目录。
// 与 real_eval_tests.rs 的 rust_host_evidence_dir() 使用同一规则
// （FOX_EVAL_OUTPUT_ROOT 优先，否则产品根下的 round 目录；历史证据目录只读）。
// O 在重型 Cargo 槽跑完 futureCommand 后，最新一份 evidence 被读取并校验身份；
// 找不到或身份不符时保持 not_run 并写明拒绝原因，绝不伪造通过。
const EVAL_ROUND_DIR = 'output/claude-design-repair-20260915/eval'
const EVAL_OUTPUT_ROOT_ENV = 'FOX_EVAL_OUTPUT_ROOT'
const HISTORICAL_EVAL_DIR = 'output/claude-design-impl-20260913/eval'
// evidence 记录必须在此窗口内生成（默认 72h），旧记录一律拒绝。
const DEFAULT_EVIDENCE_MAX_AGE_MS = 72 * 60 * 60 * 1000

function rustHostDir(productRoot) {
  const base = process.env[EVAL_OUTPUT_ROOT_ENV] || join(productRoot, EVAL_ROUND_DIR)
  if (resolve(base).startsWith(resolve(join(productRoot, HISTORICAL_EVAL_DIR)))) return null
  return join(base, 'rust-host')
}

// 同一 case 的历史记录按生成时间戳保留， newest 被采用；旧记录不覆盖、不删除。
// 文件名 `<case>-<generatedAtMs>.json`，与 real_eval_tests.rs 的写入规则一致。
async function newestEvidenceFile(dir, taskId) {
  const prefix = `${taskId.toLowerCase().replace(/-/g, '_')}-`
  let names = []
  try {
    names = (await readdir(dir)).filter((name) => name.startsWith(prefix) && name.endsWith('.json'))
  } catch {
    return null
  }
  let best = null
  let bestTs = -1
  for (const name of names) {
    const ts = Number(name.slice(prefix.length, -'.json'.length))
    if (Number.isFinite(ts) && ts > bestTs) {
      bestTs = ts
      best = name
    }
  }
  return best ? join(dir, best) : null
}

// 读取并校验 rust-host evidence。返回：
//   { status: 'none' }                       无任何记录（未跑过 cargo）
//   { status: 'rejected', rejections, path } 有记录但身份不符（评测故障）
//   { status: 'verdict', record, path }      身份校验通过，可转写 checks
async function loadRustHostEvidence({ productRoot, task, product, harnessSourceSha256, nowMs, maxAgeMs }) {
  const dir = rustHostDir(productRoot)
  if (!dir) return { status: 'none' }
  const path = await newestEvidenceFile(dir, task.id)
  if (!path) return { status: 'none' }
  let record
  try {
    record = JSON.parse(await readFile(path, 'utf8'))
  } catch (error) {
    return { status: 'rejected', path, rejections: [`evidence 解析失败：${String(error.message ?? error)}`] }
  }
  const measuredFiles = await productFileDigests(productRoot, MEASURED_PRODUCT_FILES)
  const { valid, rejections } = await validateEvidence({
    record,
    task,
    product,
    harnessSourceSha256,
    contractVersion: CONTRACT_VERSION,
    productRoot,
    nowMs,
    maxAgeMs,
    measuredFiles,
    expectedExecutionId: process.env.FOX_HARNESS_EXECUTION_ID,
  })
  if (!valid) return { status: 'rejected', path, rejections }
  return { status: 'verdict', record, path }
}

async function main() {
  const productRoot = resolve(option('--product-root', defaultProductRoot))
  const only = option('--only', null)
  const selected = only ? only.split(',').map((item) => item.trim()).filter(Boolean) : null
  // O-REVIEW-02 修复 5：未知 --only 直接报错，不能静默跑空集。
  if (only && selected.length === 0) {
    process.stderr.write(`harness-v1: --only value must name at least one task id (got "${only}")\n`)
    process.exit(2)
  }
  if (selected) {
    const known = new Set(TASKS.map((task) => task.id))
    const unknown = selected.filter((id) => !known.has(id))
    if (unknown.length > 0) {
      process.stderr.write(`harness-v1: unknown task id(s) in --only: ${unknown.join(', ')}\n`)
      process.exit(2)
    }
  }
  // O-REVIEW-03 门禁 3：baseline 只记录（允许产品失败、退出 0）；
  // acceptance 是验收门禁（失败非零退出、0 执行不构成验收）。默认严格。
  const mode = option('--mode', 'acceptance')
  if (mode !== 'baseline' && mode !== 'acceptance') {
    process.stderr.write(`harness-v1: --mode must be baseline or acceptance (got "${mode}")\n`)
    process.exit(2)
  }
  const maxAgeMs = Number(option('--evidence-max-age-ms', DEFAULT_EVIDENCE_MAX_AGE_MS))
  if (!Number.isFinite(maxAgeMs) || maxAgeMs <= 0) {
    process.stderr.write(`harness-v1: --evidence-max-age-ms must be a positive number\n`)
    process.exit(2)
  }
  const tasks = selected ? selected.map(taskById) : TASKS
  // O-REVIEW-02 修复 1：唯一 run 目录，不复用固定临时 fixture。
  const laneRoot = process.env.FOX_HARNESS_PROJECT_ROOT || join(os.tmpdir(), 'fox-harness-v1')
  const fixtureRoot = uniqueRunRoot(laneRoot)
  const outputRoot = resolve(option('--output', process.env.FOX_EVAL_OUTPUT_ROOT || join(here, 'results')))

  const product = await loadProductExecutor(productRoot)
  const code = await gitMetadata(productRoot)
  // 门禁 1：harness 来源哈希必须先算出，evidence 身份校验依赖它。
  const harnessSourceSha256 = await hashHarnessDir(here)
  const startedAt = performance.now()
  const nowMs = Date.now()
  const results = []
  const evidenceRejections = []

  for (const task of tasks) {
    const base = {
      id: task.id,
      title: task.title,
      problemIds: task.problemIds,
      tier: task.tier,
      requires: task.requires,
      tool: task.tool,
      contractRequirement: task.contractRequirement,
      expectedOutcomeAtBaseline: task.expectedOutcomeAtBaseline,
      harnessVersion: HARNESS_VERSION,
    }
    if (task.requires === 'node-executor') {
      const fixture = await buildFixture(task.fixture, FIXTURE_SPEC(task), fixtureRoot)
      const runTool = async (input, options) => product.execute(task.tool, input, options)
      const checker = CHECKERS[task.checker]
      try {
        const outcome = await checker({ task, fixtureState: fixture, runTool })
        results.push({
          ...base,
          executed: true,
          productEntry: product.entry,
          fixture: { path: fixture.path, dir: fixture.dir },
          outcome,
          baselineConfirmed:
            task.expectedOutcomeAtBaseline === 'fail'
              ? outcome.passed === false
              : task.expectedOutcomeAtBaseline === 'pass'
                ? outcome.passed === true
                : null,
          baselineExpectation: task.expectedOutcomeAtBaseline,
        })
      } catch (error) {
        results.push({ ...base, executed: true, productEntry: product.entry, outcome: { passed: false, actual: 'exception', expected: task.contractRequirement, reason: String(error.message ?? error) } })
      }
    } else {
      // rust-host 层：Node 无法编译 Rust，真实断言由 real_eval_tests.rs 在 Cargo 中
      // 执行。这里只读取其产出的 evidence 并做身份校验（门禁 1），通过后才转写 checks；
      // 未执行或身份不符一律 not_run，并把拒绝原因单列，既不算产品通过也不算产品失败。
      const loaded = await loadRustHostEvidence({
        productRoot,
        task,
        product: code,
        harnessSourceSha256,
        nowMs,
        maxAgeMs,
      })
      if (loaded.status === 'verdict') {
        const evidence = { rustHost: { ...loaded.record, path: loaded.path } }
        const outcome = CHECKERS[task.checker]({ task, evidence })
        results.push({
          ...base,
          executed: true,
          futureCommand: task.futureCommand,
          futureNote: task.futureNote,
          checker: task.checker,
          rustHostEvidence: evidence.rustHost,
          outcome,
        })
      } else if (loaded.status === 'rejected') {
        evidenceRejections.push({ id: task.id, path: loaded.path, rejections: loaded.rejections })
        results.push({
          ...base,
          executed: false,
          notRunReason: `evidence_rejected（评测故障，非产品通过也非产品失败）：${loaded.rejections.join('； ')}`,
          futureCommand: task.futureCommand,
          futureNote: task.futureNote,
          checker: task.checker,
          outcome: {
            passed: null,
            actual: 'evidence_rejected',
            expected: task.contractRequirement,
            reason: `evidence 身份校验未通过：${loaded.rejections.join('； ')}`,
          },
        })
      } else {
        results.push({
          ...base,
          executed: false,
          notRunReason: `requires=${task.requires}; product layer not compiled in this environment; future command: ${task.futureCommand}`,
          futureCommand: task.futureCommand,
          futureNote: task.futureNote,
          checker: task.checker,
          outcome: {
            passed: null,
            actual: 'not_run',
            expected: task.contractRequirement,
            reason: `${task.requires} 层未执行（需 O 在重型 Cargo 槽中跑 futureCommand，并保持产品根与当前运行一致）：${task.futureNote}`,
          },
        })
      }
    }
  }

  const executed = results.filter((item) => item.executed)
  const failed = executed.filter((item) => item.outcome.passed === false)
  const passed = executed.filter((item) => item.outcome.passed === true)
  const notRun = results.filter((item) => !item.executed)
  const failuresConfirmed = results.filter((item) => item.expectedOutcomeAtBaseline === 'fail' && item.outcome.passed === false)
  const capabilitiesConfirmed = results.filter((item) => item.expectedOutcomeAtBaseline === 'pass' && item.outcome.passed === true)
  const unexpectedPasses = results.filter((item) => item.expectedOutcomeAtBaseline === 'fail' && item.outcome.passed === true)
  const regressionsVsBaseline = results.filter((item) => item.expectedOutcomeAtBaseline === 'pass' && item.outcome.passed === false)

  // O-REVIEW-02 修复 5：0 执行 / 全部 not_run 不能作为任务验收通过。
  // O-REVIEW-03 门禁 3：acceptance 下失败必须非零退出；baseline 只记录。
  const acceptanceDecision = (function decision() {
    if (executed.length === 0) {
      return { verdict: 'not_acceptable', reason: '没有任何任务被执行（全部 not_run），不构成验收' }
    }
    if (failed.length > 0) {
      return { verdict: 'failures_present', reason: `${failed.length} 个已执行任务未通过契约` }
    }
    if (notRun.length > 0) {
      return { verdict: 'not_acceptable', reason: `${notRun.length} 个选定任务尚未执行或证据被拒，不能验收整组选定任务` }
    }
    return { verdict: 'executed_all_pass', reason: `${executed.length} 个已执行任务全部通过` }
  })()

  const harnessMeta = await harnessProvenance(here, harnessSourceSha256)

  const report = {
    kind: 'fox-harness-v1-report',
    harnessVersion: HARNESS_VERSION,
    contractVersion: CONTRACT_VERSION,
    baseProductSha: BASE_PRODUCT_SHA,
    mode,
    generatedAt: now(),
    productRoot,
    product: code,
    harness: harnessMeta,
    measuredProductFiles: MEASURED_PRODUCT_FILES,
    fixtureRoot,
    outputRoot,
    executionScope: { selected: selected ?? 'all', taskIds: tasks.map((task) => task.id) },
    durationMs: Math.round(performance.now() - startedAt),
    summary: {
      total: results.length,
      executed: executed.length,
      passed: passed.length,
      failed: failed.length,
      notRun: notRun.length,
      baselineFailuresConfirmed: failuresConfirmed.map((item) => item.id),
      baselineCapabilitiesConfirmed: capabilitiesConfirmed.map((item) => item.id),
      unexpectedPassesVsBaseline: unexpectedPasses.map((item) => item.id),
      regressionsVsBaseline: regressionsVsBaseline.map((item) => item.id),
      acceptanceDecision,
      evidenceRejections,
      untestedItems: notRun.map((item) => ({ id: item.id, reason: item.notRunReason })),
    },
    results,
    failureReasons: failed.map((item) => ({ id: item.id, reason: item.outcome.reason })),
  }

  await mkdir(outputRoot, { recursive: true })
  const target = resolve(outputRoot, 'harness-v1-results.json')
  await writeFile(target, `${JSON.stringify(report, null, 2)}\n`, 'utf8')
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  process.stdout.write(`\nharness-v1: ${passed.length} passed / ${failed.length} failed / ${notRun.length} not_run -> ${target}\n`)
  // 门禁 3：baseline 记录产品失败但退出 0；acceptance 失败退出 1、0 执行退出 3。
  // evidence 被拒绝属于评测故障，acceptance 下也退出 3（不能算验收通过）。
  if (mode === 'acceptance') {
    if (failed.length > 0) process.exitCode = 1
    else if (executed.length === 0 || notRun.length > 0 || evidenceRejections.length > 0) process.exitCode = 3
  }
}

// O-REVIEW-02 修复 5：harness 自身 provenance，与 product commit 分开记录。
// 评测代码的来源必须独立于被评测产品，否则产品改动会被误计入评测改动。
async function harnessProvenance(harnessDir, sourceSha256) {
  const { createHash } = await import('node:crypto')
  const { readFile, readdir, stat } = await import('node:fs/promises')
  const files = []
  async function walk(dir) {
    for (const name of await readdir(dir)) {
      const abs = join(dir, name)
      if ((await stat(abs)).isDirectory()) await walk(abs)
      else if (name.endsWith('.mjs') || name.endsWith('.js') || name.endsWith('.md')) files.push(abs)
    }
  }
  await walk(harnessDir)
  const digests = []
  for (const abs of files.sort()) {
    const buf = await readFile(abs)
    digests.push({ file: basename(abs), sha256: createHash('sha256').update(buf).digest('hex') })
  }
  return {
    harnessDir,
    // 门禁 1：evidence 的 harnessSourceSha256 必须与此值相等。
    sourceSha256: sourceSha256 ?? createHash('sha256').update(digests.map((item) => `${item.file}:${item.sha256}`).join('|')).digest('hex'),
    files: digests.map((item) => item.file),
    git: await gitMetadata(harnessDir).catch(() => ({ commit: null, branch: null })),
    note: 'harness 自身来源 hash；与 product commit 独立。product 改动不改变此值。evidence 身份校验要求此值与 Rust 侧记录一致。',
  }
}

function basename(path) {
  return path.split(/[\\/]/).pop()
}
function FIXTURE_SPEC(task) {
  const specs = {
    bigLines: { kind: 'text', lineCount: 5000, linePrefix: 'line-' },
    unicode: { kind: 'text', text: 'A😀Fox' },
    crlf: { kind: 'text', text: 'one\r\ntwo\r\n' },
    manyMatchFiles: { kind: 'tree', files: 300, needle: 'probe-needle', perFile: 1 },
  }
  if (!(task.fixture in specs)) throw new Error(`unknown fixture: ${task.fixture}`)
  return specs[task.fixture]
}

main().catch((error) => {
  console.error(`harness-v1 runner failed: ${error.stack ?? error}`)
  process.exit(2)
})
