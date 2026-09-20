// 独立 checker（harness-v1.0）。
// 原则：预期值由 fixture 状态独立重算，不引用模型自评、不从修改后输出反推。
// 每个 checker 接收 { task, fixtureState, results }，返回 { passed, actual, expected, reason }。

import { readFile } from 'node:fs/promises'

function wellFormedUtf16(text) {
  // 按 UTF-16 码元迭代：high surrogate 后必跟 low surrogate，low 不得单独出现。
  // 消费一对后跳过 low 半部，不把它误判为 lone surrogate。
  let index = 0
  while (index < text.length) {
    const code = text.charCodeAt(index)
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(index + 1)
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false
      index += 2
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return false
    } else {
      index += 1
    }
  }
  return true
}

function textOf(result) {
  return (result?.content ?? []).map((part) => part.text ?? '').join('')
}

function detailsOf(result) {
  return result?.details ?? {}
}

// E-T-03：以固定 limit 循环读取并重建。ground truth 优先取 fixture 写入时记录的文本，
// 否则直接读原文件；两者都不可得时报错，不接受工具自身输出作为真值。
export async function checkPaginationRoundTrip({ task, fixtureState, runTool }) {
  const limit = task.input.limit
  let expected
  if (typeof fixtureState?.text === 'string') {
    expected = fixtureState.text
  } else if (fixtureState?.path) {
    expected = await readFile(fixtureState.path, 'utf8')
  } else {
    return { passed: false, actual: 'no ground truth', expected: 'fixture text or path', reason: 'checker 缺少独立真值来源' }
  }
  let offset = 0
  let reassembled = ''
  let pages = 0
  let lastTruncated = null
  // 上限防止死循环（即使产品方有缺陷也能终止）。
  while (pages < 10000) {
    const result = await runTool({ path: fixtureState.path, offset, limit })
    const text = textOf(result)
    const details = detailsOf(result)
    reassembled += text
    pages += 1
    lastTruncated = details.truncated
    if (details.truncated !== true) break
    if (text.length === 0) {
      return { passed: false, actual: 'empty page while truncated', expected: 'non-empty pages until end', reason: 'truncated=true 但返回空文本，无法继续' }
    }
    offset += text.length
  }
  const exact = reassembled === expected
  const tailOk = lastTruncated === false
  const passed = exact && tailOk
  return {
    passed,
    actual: { pages, reassembledLength: reassembled.length, finalTruncated: lastTruncated, exact },
    expected: { reassembledLength: expected.length, finalTruncated: false, exact: true },
    reason: passed
      ? '分页重建逐字节等于原文件，末页 truncated=false'
      : !exact
        ? '重建文本与原文件不一致'
        : '末页 truncated 未为 false',
  }
}

// E-T-03b：落在代理对内部的 offset 不得返回半截代理对。
export async function checkUnicodeBoundary({ task, fixtureState, runTool }) {
  const result = await runTool({ ...task.input, ...(fixtureState?.path ? { path: fixtureState.path } : {}) })
  const text = textOf(result)
  const wellFormed = wellFormedUtf16(text)
  return {
    passed: wellFormed,
    actual: { returned: text, codeUnits: Array.from(text, (c) => 'U+' + c.charCodeAt(0).toString(16).toUpperCase().padStart(4, '0')).join(' '), wellFormed },
    expected: { wellFormed: true },
    reason: wellFormed ? '返回文本良构' : '返回包含 lone surrogate（半截代理对），边界不安全',
  }
}

// O-REVIEW-02 修复 4：识别"拒绝"的语义。任意异常不算正确拒绝——
// 一个永远抛 EIO 的坏执行器也会通过。只有明确的参数校验拒绝才算。
// 产品约定的失败结果（Ok + isError）也按拒绝处理；I/O/权限/崩溃类异常不算。
function classifyRejection(result, error) {
  if (result !== undefined) {
    const details = detailsOf(result)
    // 产品约定：失败结果通过 Ok + isError=true 返回（CONTRACTS EX-v1）。
    if (result.isError === true) {
      return { rejected: true, kind: 'contract_failure_result', detail: { isError: true, details } }
    }
    return { rejected: false, kind: 'accepted', detail: { details, head: textOf(result).slice(0, 24) } }
  }
  const message = String(error?.message ?? error)
  // 无关故障：文件不存在 / 权限 / 磁盘 / 执行器崩溃。这些不是参数校验拒绝。
  const infraPattern = /ENOENT|EACCES|EIO|EPERM|not such file|permission|undefined|null/i
  if (infraPattern.test(message)) {
    return { rejected: false, kind: 'infra_error', detail: { error: message } }
  }
  // 其余异常按约定视为工具拒绝（产品当前以 throw 表达校验失败）。
  return { rejected: true, kind: 'thrown_rejection', detail: { error: message } }
}

// E-T-04a：缺失/负数/小数参数必须被拒绝；有效输入必须成功。
// 注意：必须传入真实 fixture path；否则缺 path 的无关错误会被误当成参数校验拒绝。
export async function checkLineModePairing({ task, fixtureState, runTool }) {
  const outcomes = []
  for (const item of task.inputs) {
    let result = undefined
    let thrown = null
    try {
      result = await runTool({ ...item.args, path: fixtureState.path })
    } catch (error) {
      thrown = error
    }
    // 有效输入正例：必须成功（非 rejected）。
    if (item.kind === 'valid') {
      const ok = result !== undefined && result.isError !== true && thrown === null
      outcomes.push({ label: item.label, kind: ok ? 'valid_ok' : 'valid_failed', rejected: !ok, detail: thrown ? { error: String(thrown.message ?? thrown) } : { details: detailsOf(result) } })
      continue
    }
    const verdict = classifyRejection(result, thrown)
    outcomes.push({ label: item.label, ...verdict })
  }
  const validCases = outcomes.filter((item) => item.label.startsWith('valid'))
  // 有效输入正例：若任务定义了正例，必须全部成功；无正例时此项不阻拦。
  const validOk = validCases.length === 0 || validCases.every((item) => item.kind === 'valid_ok')
  const illegal = outcomes.filter((item) => item.kind !== 'valid' && item.kind !== 'valid_ok' && item.kind !== 'valid_failed')
  const allRejected = illegal.every((item) => item.rejected)
  const noInfra = illegal.every((item) => item.kind !== 'infra_error')
  const passed = validOk && allRejected && noInfra
  return {
    passed,
    actual: outcomes.map((item) => `${item.label}: ${item.kind}`),
    expected: [
      ...task.inputs.filter((item) => item.kind === 'valid').map((item) => `${item.label}: valid_ok`),
      ...task.inputs.filter((item) => item.kind !== 'valid').map((item) => `${item.label}: rejected(contract_failure_result 或 thrown_rejection)`),
    ],
    reason: !validOk
      ? `有效输入正例未成功（${validCases.map((item) => item.label).join('、')}），执行器可能一律拒绝或故障`
      : !noInfra
        ? `${illegal.filter((item) => item.kind === 'infra_error').map((item) => item.label).join('、')} 的异常属于无关故障（ENOENT/EIO/权限等），不能充当参数校验拒绝`
        : !allRejected
          ? illegal.filter((item) => !item.rejected).map((item) => `${item.label} 未被拒绝（默默补默认值）`).join('；')
          : '有效输入成功，全部非法参数被拒绝，且无无关故障冒充拒绝',
  }
}

// E-T-04b：被上限丢弃的行页不得声称完成。
export async function checkLinePageComplete({ task, fixtureState, runTool }) {
  const result = await runTool({ ...task.input, ...(fixtureState?.path ? { path: fixtureState.path } : {}) }, { limits: task.limits })
  const details = detailsOf(result)
  const hasField = typeof details.pageComplete === 'boolean'
  const returned = textOf(result)
  const requestedChars = fixtureState.text.length
  const discarded = returned.length < requestedChars
  const passed = hasField && details.pageComplete === false
  return {
    passed,
    actual: { pageComplete: details.pageComplete ?? '<absent>', hasField, returnedLength: returned.length },
    expected: { pageComplete: false, hasField: true },
    reason: !hasField
      ? 'details 缺少 pageComplete 字段'
      : `pageComplete=${details.pageComplete}，但请求的 ${requestedChars} 字符仅返回 ${returned.length}（被上限丢弃）`,
  }
}

// E-T-04c：扫描/匹配/上限计数的区分字段必须存在且语义正确。
// O-REVIEW-02 修复 2：必须传 fixtureState.path，否则执行器因 undefined path 失败，
// 那是评测故障而非产品搜索缺陷。
// O-REVIEW-02 修复 3：matchLimitReached=true 不无条件推出 scanComplete=false。
// 「完整扫描+返回裁剪」（totalMatches 已知且 > returnedCount）与「扫描预算耗尽」
// （scanComplete=false）是两种不同情形，都合法；checker 只校验一致性。
export async function checkSearchFieldDistinction({ task, fixtureState, runTool }) {
  const result = await runTool({ ...task.input, ...(fixtureState?.path ? { path: fixtureState.path } : {}) }, { limits: task.limits })
  const details = detailsOf(result)
  const required = ['scanComplete', 'scanTruncated', 'matchLimitReached', 'returnedCount', 'totalMatches']
  const missing = required.filter((name) => !(name in details))
  const limitReached = details.matchLimitReached === true
  const countConsistent = typeof details.returnedCount === 'number' && details.returnedCount <= task.limits.maxMatches
  const unknownTotalOk = details.totalMatches === null || typeof details.totalMatches === 'number'

  // 语义一致性：不允许同时声称"扫描完成且未截断"却"命中上限"（自相矛盾）。
  const contradiction = details.scanComplete === true && details.scanTruncated !== true && limitReached && details.totalMatches === null

  // 完整扫描+返回裁剪：totalMatches 已知且大于 returnedCount —— 合法。
  const completeScanCapped = details.scanComplete === true && typeof details.totalMatches === 'number' && details.totalMatches > details.returnedCount
  // 扫描预算耗尽：scanComplete=false —— 合法。
  const budgetExhausted = details.scanComplete === false

  const passed = missing.length === 0 && limitReached && countConsistent && unknownTotalOk && !contradiction
  return {
    passed,
    actual: {
      present: required.filter((name) => name in details),
      values: Object.fromEntries(required.map((name) => [name, details[name] ?? '<absent>'])),
      interpretation: completeScanCapped ? '完整扫描+返回裁剪' : budgetExhausted ? '扫描预算耗尽' : contradiction ? '自相矛盾' : '其它',
    },
    expected: { present: required, matchLimitReached: true, returnedCount: '<= maxMatches', totalMatches: 'null 或数字', scanSemantics: '完整扫描+裁剪 或 预算耗尽，二者一致' },
    reason: missing.length > 0
      ? `缺少字段：${missing.join(', ')}`
      : !limitReached
        ? '命中上限时 matchLimitReached 未为 true'
        : !countConsistent
          ? `returnedCount=${details.returnedCount} 超过 maxMatches=${task.limits.maxMatches}`
          : !unknownTotalOk
            ? 'totalMatches 既非 null 也非数字'
            : contradiction
              ? 'scanComplete=true 且 scanTruncated!=true 且 matchLimitReached=true 且 totalMatches=null：语义自相矛盾'
              : '字段齐备且语义一致',
  }
}

// 以下为 rust-host 层任务的 checker。真实断言在 real_eval_tests.rs 的可运行用例中
// 实现（真实执行器 + 独立重算 + 持久化/回读），Node 层不执行 Cargo，只把已通过身份
// 校验的 evidence（见 run-harness.loadRustHostEvidence / evidence-identity.mjs）
// 机器可读地转写为 harness 行：结论只取自 checks，cargo 退出码不参与判定。
// 再次自检 overall 与 checks 的一致性与非空性，作为纵深防御。
// 入口性质（O-REVIEW-03 门禁 4）：这些用例是直接 Host/DB/reader 定向 Rust 测试，
// model_receipt 只是该入口的局部投影；不驱动 Node 模型 worker，也不构成完整
// synthetic-provider→Kernel→Office 链。既有 real_task_evaluation* 入口按预算/队列
// 另行验证，本 harness 不覆盖、不替代。
function summarizeRustHostEvidence({ task, evidence }) {
  const rustHost = evidence?.rustHost
  if (!rustHost) {
    return {
      passed: null,
      actual: 'not_run',
      expected: task.contractRequirement,
      reason: evidence?.notRunReason ?? 'rust-host 用例未执行（需要 O 授予的重型 Cargo 槽）',
    }
  }
  const checks = Array.isArray(rustHost.checks) ? rustHost.checks : []
  if (checks.length === 0) {
    return {
      passed: false,
      actual: { overall: rustHost.overall, checks: [] },
      expected: task.contractRequirement,
      reason: 'evidence checks 为空：0 检查的记录不能算通过',
    }
  }
  const failed = checks.filter((row) => !row.passed)
  const consistent = rustHost.overall === (failed.length === 0 ? 'passed' : 'failed')
  const passed = rustHost.overall === 'passed' && failed.length === 0 && consistent
  return {
    passed,
    actual: {
      overall: rustHost.overall,
      failedChecks: failed.map((row) => row.id),
      checks,
    },
    expected: task.contractRequirement,
    reason: !consistent
      ? `overall=${rustHost.overall} 与 checks 结果不一致（${failed.length} 项失败）`
      : failed.length === 0
          ? `rust-host 用例通过：${task.futureCommand}`
          : `rust-host 用例有 ${failed.length} 项未过：${failed.map((row) => `${row.id}（${row.detail}）`).join('； ')}`,
  }
}

export const checkCommandStdoutOnlyFailure = summarizeRustHostEvidence
export const checkLegalDeletion = summarizeRustHostEvidence
export const checkPermissionDenial = summarizeRustHostEvidence

export const CHECKERS = {
  checkPaginationRoundTrip,
  checkUnicodeBoundary,
  checkLineModePairing,
  checkLinePageComplete,
  checkSearchFieldDistinction,
  checkCommandStdoutOnlyFailure,
  checkLegalDeletion,
  checkPermissionDenial,
}
