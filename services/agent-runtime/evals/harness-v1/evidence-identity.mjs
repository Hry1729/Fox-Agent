// 证据身份绑定（harness-v1.2 / O-REVIEW-03 门禁 1）。
//
// rust-host 任务的真实断言由 real_eval_tests.rs 在 Cargo 中执行，产出的 evidence
// JSON 再由本 harness 转写。O-REVIEW-03 指出：只看文件名、overall 字符串和 checks
// 数组不足以证明该记录就是"当前产品上本次执行的结论"。因此 evidence 必须与
//   生产（产品来源）/ 评测（harness 来源）/ 契约 / 用例 / 本次执行标识 / 源码 dirty 哈希
// 绑定，任一缺失或不匹配都拒绝，且 0 检查的记录绝不能算通过。
//
// 本文件是 Node 与 Rust 两侧共享的唯一真值：real_eval_tests.rs 的
// record_rust_host_case() 用完全相同的字段、算法和分隔符生成绑定值，本文件按同样
// 规则校验。两侧任何一方改动都必须同步另一方并更新 HARNESS_VERSION。

import { createHash } from 'node:crypto'
import { readFile, readdir, stat } from 'node:fs/promises'
import { basename, join } from 'node:path'

// rust-host 用例实际度量其行为的产品源文件（相对产品根）。content 哈希会捕获
// 未提交改动，因此 commit 相同但工作区不同时绑定值也不同（"源码 dirty 哈希"）。
// 两侧必须逐字一致。
export const MEASURED_PRODUCT_FILES = [
  'apps/desktop/src-tauri/src/tool_host.rs',
  'apps/desktop/src-tauri/src/resource_gateway.rs',
  'apps/desktop/src-tauri/src/tool_guard.rs',
  'apps/desktop/src-tauri/src/runtime_host/mod.rs',
  'apps/desktop/src-tauri/src/database/repositories.rs',
  'apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/real_eval_tests.rs',
]

// 执行标识：把四项身份拼成一个可复现的字符串。两侧用同一拼法，不做哈希以避免
// 跨语言哈希实现差异；productSha/harnessSourceSha256 本身已是密码学绑定。
export function executionStampOf({ productSha, harnessSourceSha256, contractVersion, caseId }) {
  return [productSha, harnessSourceSha256, contractVersion, caseId].join('|')
}

// harness 目录来源哈希。规则：递归收集 *.mjs/*.js/*.md，按完整路径排序，
// 逐文件 sha256（原始字节），拼成 `basename:sha256` 用 `|` 连接后再取一次 sha256。
// real_eval_tests.rs::harness_source_sha256() 用同一规则。
export async function hashHarnessDir(harnessDir) {
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
    digests.push(`${basename(abs)}:${createHash('sha256').update(buf).digest('hex')}`)
  }
  return createHash('sha256').update(digests.join('|')).digest('hex')
}

// 产品源文件的 content 哈希清单。任一文件缺失或内容不同都意味着"另一个产品"。
export async function productFileDigests(productRoot, files = MEASURED_PRODUCT_FILES) {
  const out = []
  for (const rel of files) {
    const abs = join(productRoot, rel)
    let sha = null
    try {
      const buf = await readFile(abs)
      sha = createHash('sha256').update(buf).digest('hex')
    } catch {
      sha = null
    }
    out.push({ path: rel, sha256: sha })
  }
  return out
}

// 校验一份 evidence 记录是否就是"当前产品上本次执行的结论"。
// 返回 { valid, rejections: [] }。rejections 非空时 valid 为 false，
// 调用方必须把该记录当作未执行（评测故障），既不算产品通过也不算产品失败。
export async function validateEvidence({
  record,
  task,
  product,                 // { commit, branch, dirty } 来自 gitMetadata(productRoot)
  harnessSourceSha256,     // 本次运行 harness 的来源哈希
  contractVersion,
  productRoot,
  nowMs = Date.now(),
  maxAgeMs,                // 记录必须在此窗口内生成；默认 72h
  measuredFiles,           // 期望的产品文件哈希清单（来自 productFileDigests）
  expectedExecutionId,     // 显式关联同一批 Rust 运行与本次导入，不复用旧运行。
}) {
  const rejections = []
  const fail = (reason) => rejections.push(reason)

  if (!record || typeof record !== 'object' || Array.isArray(record)) return { valid: false, rejections: ['evidence 不是对象'] }

  for (const field of ['case', 'contractVersion', 'harness', 'productSha', 'harnessSourceSha256', 'executionStamp']) {
    if (typeof record[field] !== 'string' || record[field].length === 0) fail(`${field} 必须是非空字符串`)
  }
  if (typeof expectedExecutionId !== 'string' || expectedExecutionId.trim() === '') fail('缺少本次 expectedExecutionId')
  if (typeof record.executionId !== 'string' || record.executionId !== expectedExecutionId) fail('executionId 与本次运行不匹配')
  if (typeof record.productSha !== 'string' || !/^[0-9a-f]{40}$/.test(record.productSha)) fail('productSha 格式非法')
  if (typeof product?.commit !== 'string' || !/^[0-9a-f]{40}$/.test(product.commit)) fail('当前产品 commit 格式非法')
  if (typeof record.harnessSourceSha256 !== 'string' || !/^[0-9a-f]{64}$/.test(record.harnessSourceSha256)) fail('harnessSourceSha256 格式非法')
  if (!Array.isArray(record.productFiles)) fail('productFiles 必须是数组')
  if (!Array.isArray(measuredFiles) || measuredFiles.length === 0) fail('本次产品文件清单缺失')

  // 1. 身份字段必须齐备（缺失即拒绝）。
  const required = ['case', 'contractVersion', 'harness', 'productSha', 'harnessSourceSha256', 'executionStamp', 'generatedAtMs', 'overall', 'checks', 'productFiles']
  for (const key of required) {
    if (record[key] === undefined || record[key] === null) fail(`缺少身份字段 ${key}`)
  }

  // 2. 用例必须对上（错 case 拒绝）。
  if (typeof record.case === 'string' && record.case !== task.id) fail(`case 不匹配：记录为 ${record.case}，任务为 ${task.id}`)

  // 3. 契约版本必须对上。
  if (typeof record.contractVersion === 'string' && record.contractVersion !== contractVersion) fail(`contractVersion 不匹配：${record.contractVersion} != ${contractVersion}`)

  // 4. 评测来源必须对上（用旧 checker 产生的记录不能冒充当前 checker）。
  if (typeof record.harnessSourceSha256 === 'string' && record.harnessSourceSha256 !== harnessSourceSha256) fail(`harnessSourceSha256 不匹配：${record.harnessSourceSha256} != ${harnessSourceSha256}`)

  // 5. 生产标识：必须是非零 SHA，且等于本次运行的产品 commit。
  const zeroSha = /^0+$/
  if (typeof record.productSha === 'string') {
    if (zeroSha.test(record.productSha)) fail('productSha 全零')
    if (product?.commit && record.productSha !== product.commit) fail(`productSha 不匹配：${record.productSha} != ${product.commit}`)
  }
  if (!product?.commit) fail('本次运行无法解析产品 commit，不能接受任何 evidence')
  else if (zeroSha.test(product.commit)) fail('本次运行产品 commit 全零')

  // 6. 源码 dirty 哈希：产品文件清单必须逐个 content 相等（错产品/旧产品/脏改动不一致）。
  if (Array.isArray(record.productFiles) && Array.isArray(measuredFiles)) {
    if (record.productFiles.length !== measuredFiles.length) fail(`productFiles 数量不符：${record.productFiles.length} != ${measuredFiles.length}`)
    const byPath = new Map(measuredFiles.map((row) => [row.path, row.sha256]))
    const seen = new Set()
    for (const row of record.productFiles) {
      if (!row || typeof row.path !== 'string' || typeof row.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(row.sha256)) {
        fail('productFiles 行格式非法或源文件不可读')
        continue
      }
      if (seen.has(row.path)) fail(`productFiles 重复路径 ${row.path}`)
      seen.add(row.path)
      const want = byPath.get(row.path)
      if (want === undefined) fail(`productFiles 含未知路径 ${row.path}`)
      else if (row.sha256 !== want) fail(`productFiles 内容不一致：${row.path}`)
    }
    for (const [path, digest] of byPath) {
      if (!seen.has(path)) fail(`productFiles 缺少路径 ${path}`)
      if (typeof digest !== 'string' || !/^[0-9a-f]{64}$/.test(digest)) fail(`本次源文件不可读 ${path}`)
    }
  }

  // 7. 执行标识必须等于当前四项身份的拼接。
  const expectedStamp = executionStampOf({ productSha: product?.commit ?? '', harnessSourceSha256, contractVersion, caseId: task.id })
  if (typeof record.executionStamp === 'string') {
    if (record.executionStamp !== expectedStamp) fail('executionStamp 与当前运行身份不匹配（旧记录或另一产品）')
  } else {
    fail('executionStamp 缺失')
  }

  // 8. 时间：拒绝 1970/空时间，并要求落在新鲜窗口内（旧记录拒绝）。
  const ts = record.generatedAtMs
  if (!Number.isSafeInteger(ts)) fail('generatedAtMs 必须是整数时间戳')
  if (!Number.isFinite(ts) || ts < 978307200000) fail(`generatedAtMs 非法（${record.generatedAtMs}）`)
  else if (maxAgeMs !== undefined && ts < nowMs - maxAgeMs) fail(`记录过期：生成于 ${new Date(ts).toISOString()}，超出 ${maxAgeMs}ms 窗口`)
  else if (ts > nowMs + 60_000) fail(`generatedAtMs 指向未来（${new Date(ts).toISOString()}）`)

  // 9. 检查集：必须非空、id 齐全且唯一、必需检查一个不能少。
  const checks = Array.isArray(record.checks) ? record.checks : []
  if (checks.length === 0) fail('checks 为空：0 检查的记录不能算通过')
  for (const row of checks) {
    if (!row || typeof row.id !== 'string' || row.id === '' || typeof row.passed !== 'boolean') fail('checks 行必须含非空id和布尔passed')
  }
  const ids = checks.map((row) => row?.id)
  const dup = ids.filter((id, index) => id !== undefined && ids.indexOf(id) !== index)
  if (dup.length > 0) fail(`checks id 重复：${[...new Set(dup)].join(', ')}`)
  if (task.requiredChecks && task.requiredChecks.length > 0) {
    const have = new Set(ids.filter(Boolean))
    const missing = task.requiredChecks.filter((id) => !have.has(id))
    if (missing.length > 0) fail(`缺少必需检查：${missing.join(', ')}`)
  }

  // 10. 矛盾计数：overall 必须与 checks 一致，cargo 退出码不能掩盖 checks 内失败。
  const failed = checks.filter((row) => row?.passed === false)
  if (record.overall === 'passed' && failed.length > 0) fail(`overall=passed 但有 ${failed.length} 项检查失败（矛盾）`)
  if (record.overall === 'failed' && failed.length === 0) fail('overall=failed 但无任何失败检查（矛盾）')
  if (record.overall !== 'passed' && record.overall !== 'failed') fail(`overall 取值非法：${record.overall}`)

  return { valid: rejections.length === 0, rejections }
}
