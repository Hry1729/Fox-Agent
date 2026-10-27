// Collect what the isolated run actually left on disk.
//
// This script reports facts and never decides a scenario: the pass/fail of a
// desktop scenario (identity round-trip, restart continuation, cancel, ...) is
// the operator's reading of the UI plus these facts. Anything it cannot read is
// reported as "unverified", never as a pass.
//
// Usage: node evidence.mjs [--out <file>]
import { copyFileSync, existsSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import {
  DATA_DIR, EVIDENCE_DIR, FIXTURE, FIXTURE_DIR, RUN_ROOT, ensureRunDirs, isAlive, nowIso, readState,
} from './lib/lifecycle.mjs'

const args = process.argv.slice(2)
const value = (name) => {
  const index = args.indexOf(name)
  return index >= 0 ? args[index + 1] : undefined
}

ensureRunDirs()
const state = readState()
const lines = []
const say = (text = '') => lines.push(text)

function walk(root, depth = 0, acc = []) {
  if (depth > 3 || !existsSync(root)) return acc
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const full = join(root, entry.name)
    let size = 0
    try { size = statSync(full).size } catch {}
    acc.push({ path: relative(RUN_ROOT, full).split('\\').join('/'), directory: entry.isDirectory(), size })
    if (entry.isDirectory()) walk(full, depth + 1, acc)
  }
  return acc
}

say(`# 单 Pi 主线验收证据 · ${nowIso()}`)
say()
say(`- 隔离根目录：\`${RUN_ROOT}\``)
say(`- 数据目录：\`${DATA_DIR}\``)
say(`- 本次运行记录的进程：${state.runs.length}`)
for (const run of state.runs) {
  say(`  - pid ${run.pid} · 启动 ${run.startedAt} · ${isAlive(run.pid) ? '仍在运行' : '已退出'} · ${run.binary}`)
}
say()

// ---- 1. database facts (read-only copy, never the live file)
const databasePath = join(DATA_DIR, 'facts.db')
const snapshotPath = join(EVIDENCE_DIR, `facts-${Date.now()}.db`)
let databaseFacts = null
if (existsSync(databasePath)) {
  copyFileSync(databasePath, snapshotPath)
  try {
    const { DatabaseSync } = await import('node:sqlite')
    const db = new DatabaseSync(snapshotPath, { readOnly: true })
    const query = (sql) => {
      try { return db.prepare(sql).all() } catch (error) { return { error: String(error.message) } }
    }
    databaseFacts = {
      drivers: query('SELECT agent_backend, COUNT(*) AS total FROM conversations GROUP BY agent_backend'),
      engines: query('SELECT engine_id, COUNT(*) AS total FROM kernel_runs GROUP BY engine_id'),
      sessions: query("SELECT id, conversation_id, runtime_type FROM runtime_sessions"),
    }
    db.close()
  } catch (error) {
    databaseFacts = { error: `node:sqlite unavailable: ${error.message}` }
  }
  say('## 1. 数据库事实（只读副本）')
  say()
  say('```json')
  say(JSON.stringify(databaseFacts, null, 2))
  say('```')
  say()
} else {
  say('## 1. 数据库事实')
  say()
  say('- 未找到 `facts.db`：应用可能尚未启动，或数据目录不是本次运行的目录。**未验证**。')
  say()
}

// ---- 2. session files
const files = walk(RUN_ROOT)
const sessionFiles = files.filter((file) => !file.directory && file.path.endsWith('.jsonl'))
say('## 2. 会话文件')
say()
if (sessionFiles.length === 0) {
  say('- 没有找到 Pi 原生 JSONL 会话文件。阶段二的“原生会话持久化”**未验证**。')
} else {
  say(`- 找到 ${sessionFiles.length} 个 JSONL 会话文件（应位于 Fox 管理的用户数据目录内）：`)
  for (const file of sessionFiles) say(`  - \`${file.path}\` · ${file.size} 字节`)
}
say()

// ---- 3. scenario checklist from the fixture
const labels = {
  'single-driver': '单驱动准入',
  'expert-round-trip': '专家连续对话',
  'knowledge-qa': '资料问答',
  restart: '完整重启',
  interrupt: '进程中断',
  'context-reset': '上下文重置',
  'cross-expert': '跨专家隔离',
  'compaction-restart': '压缩后重启',
  cancel: '取消与迟到事件',
  'corrupt-session': '会话损坏',
  'windows-path': 'Windows 路径',
}
say('## 3. 场景清单（默认未验证）')
say()
say('| 场景 | 操作 | 通过条件 | 结果 |')
say('| --- | --- | --- | --- |')
for (const scenario of FIXTURE.scenarios) {
  say(`| ${labels[scenario.id] ?? scenario.id} | ${scenario.operation} | ${scenario.pass} | 未验证 |`)
}
say()
say('> 结果一栏只能由执行人员在完成 UI 观察后填写。脚本不替任何一个场景判定通过。')
say()

// ---- 4. fixture identities, for the operator's answer key
say('## 4. 夹具身份与答案键')
say()
for (const expert of FIXTURE.experts) {
  say(`- ${expert.displayName} → 资料库「${expert.knowledgeBase.name}」`)
}
for (const question of FIXTURE.questions) {
  say(`  - ${question.expert}：${question.question} → 期望事实 ${question.expectedFact}，来源 ${question.expectedSource}，引用 ${question.expectedCitation}`)
}
say()
say(`- 需要的资料工具：${FIXTURE.requiredTools.join('、')}`)
say(`- 不得启用的驱动：${FIXTURE.forbiddenDrivers.join('、')}`)

const out = value('--out') ?? join(EVIDENCE_DIR, `evidence-${Date.now()}.md`)
writeFileSync(out, lines.join('\n'), 'utf8')
console.log(out)
