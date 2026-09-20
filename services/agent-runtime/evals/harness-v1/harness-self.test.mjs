// checker 自身正确性测试（harness-v1.2）。
// 这些只检验 checker 与门禁的判别力：拒绝坏输出、接受好输出。**不算产品能力通过。**
import test from 'node:test'
import assert from 'node:assert/strict'
import {
  checkPaginationRoundTrip,
  checkUnicodeBoundary,
  checkLineModePairing,
  checkLinePageComplete,
  checkSearchFieldDistinction,
  checkCommandStdoutOnlyFailure,
} from './checkers.mjs'
import {
  executionStampOf,
  validateEvidence,
  MEASURED_PRODUCT_FILES,
} from './evidence-identity.mjs'
import { TASKS } from './tasks.mjs'

// 伪造的 runTool：按 fixture 模拟产品行为，用来检验 checker 判别力。
function fakeRunner(behavior) {
  return async (input) => behavior(input)
}

test('checker harness: pagination accepts an exact round trip', async () => {
  const text = 'abc\ndef\nghi\n'
  const runTool = async (input) => {
    const slice = text.slice(input.offset, input.offset + input.limit)
    const truncated = input.offset + input.limit < text.length
    return { content: [{ type: 'text', text: slice }], details: { truncated } }
  }
  const outcome = await checkPaginationRoundTrip({
    task: { input: { limit: 4 } },
    fixtureState: { text },
    runTool,
  })
  assert.equal(outcome.passed, true)
  assert.equal(outcome.actual.exact, true)
})

test('checker harness: pagination rejects a lossy round trip', async () => {
  const text = 'abc\ndef\nghi\n'
  const runTool = async (input) => {
    // 假实现：每页悄悄丢一个字符
    const slice = text.slice(input.offset, input.offset + input.limit).slice(1)
    const truncated = input.offset + input.limit < text.length
    return { content: [{ type: 'text', text: slice }], details: { truncated } }
  }
  const outcome = await checkPaginationRoundTrip({
    task: { input: { limit: 4 } },
    fixtureState: { text },
    runTool,
  })
  assert.equal(outcome.passed, false)
})

test('checker harness: pagination rejects truncated=true forever (no progress)', async () => {
  const runTool = async () => ({ content: [{ type: 'text', text: 'x' }], details: { truncated: true } })
  const outcome = await checkPaginationRoundTrip({
    task: { input: { limit: 4 } },
    fixtureState: { text: 'abc\ndef\n' },
    runTool,
  })
  assert.equal(outcome.passed, false)
  assert.match(outcome.reason, /空文本|不一致|truncated/)
})

test('checker harness: unicode boundary accepts well-formed text', async () => {
  const outcome = await checkUnicodeBoundary({
    task: { input: { offset: 1, limit: 2 } },
    runTool: fakeRunner(() => ({ content: [{ type: 'text', text: '😀' }], details: {} })),
  })
  assert.equal(outcome.passed, true)
})

test('checker harness: unicode boundary rejects a lone surrogate', async () => {
  const outcome = await checkUnicodeBoundary({
    task: { input: { offset: 2, limit: 2 } },
    runTool: fakeRunner(() => ({ content: [{ type: 'text', text: '\ude00F' }], details: {} })),
  })
  assert.equal(outcome.passed, false)
  assert.match(outcome.reason, /lone surrogate/)
})

test('checker harness: line pairing rejects an executor that always throws EIO (O-REVIEW-02 fix 4)', async () => {
  const task = {
    inputs: [
      { label: 'valid-pair', args: { startLine: 1, lineCount: 1 }, kind: 'valid' },
      { label: 'missing-lineCount', args: { startLine: 1 }, kind: 'illegal' },
    ],
  }
  const fixtureState = { path: 'fake-but-present' }
  // 永远抛 EIO 的坏执行器：有效输入也失败，必须判为不通过。
  const broken = fakeRunner(() => { throw new Error('EIO unrelated disk failure') })
  const outcome = await checkLineModePairing({ task, fixtureState, runTool: broken })
  assert.equal(outcome.passed, false, JSON.stringify(outcome))
  assert.match(outcome.reason, /有效输入正例未成功|无关故障/)
})

test('checker harness: line pairing accepts proper reject/accept mix', async () => {
  const task = {
    inputs: [
      { label: 'valid-pair', args: { startLine: 1, lineCount: 1 }, kind: 'valid' },
      { label: 'missing-lineCount', args: { startLine: 1 }, kind: 'illegal' },
      { label: 'negative', args: { startLine: -1, lineCount: 1 }, kind: 'illegal' },
    ],
  }
  const fixtureState = { path: 'fake-but-present' }
  const mixed = fakeRunner(async (input) => {
    if (input.lineCount === undefined || input.startLine < 0 || !Number.isInteger(input.lineCount)) {
      throw new Error('invalid line params')
    }
    return { content: [{ type: 'text', text: 'ok' }], details: {} }
  })
  const outcome = await checkLineModePairing({ task, fixtureState, runTool: mixed })
  assert.equal(outcome.passed, true, JSON.stringify(outcome))
})

test('checker harness: line pairing treats Ok+isError as contract rejection', async () => {
  const task = { inputs: [{ label: 'fractional', args: { startLine: 1, lineCount: 1.5 }, kind: 'illegal' }] }
  const fixtureState = { path: 'fake-but-present' }
  const contractFail = fakeRunner(() => ({ isError: true, content: [{ type: 'text', text: 'lineCount must be integer' }], details: {} }))
  const outcome = await checkLineModePairing({ task, fixtureState, runTool: contractFail })
  assert.equal(outcome.passed, true, JSON.stringify(outcome))
})

test('checker harness: page complete requires the field and false when discarding', async () => {
  const task = { input: { startLine: 1, lineCount: 2 }, limits: { maxReadChars: 2 } }
  const fixtureState = { text: 'one\r\ntwo\r\n' }
  const good = fakeRunner(() => ({ content: [{ type: 'text', text: 'on' }], details: { pageComplete: false, truncated: true } }))
  const missingField = fakeRunner(() => ({ content: [{ type: 'text', text: 'on' }], details: { truncated: true } }))
  const lying = fakeRunner(() => ({ content: [{ type: 'text', text: 'on' }], details: { pageComplete: true } }))
  assert.equal((await checkLinePageComplete({ task, fixtureState, runTool: good })).passed, true)
  assert.equal((await checkLinePageComplete({ task, fixtureState, runTool: missingField })).passed, false)
  assert.equal((await checkLinePageComplete({ task, fixtureState, runTool: lying })).passed, false)
})

test('checker harness: search distinction requires all fields, path passthrough, and both scan semantics', async () => {
  const task = { input: { pattern: 'x' }, limits: { maxMatches: 200 } }

  // O-REVIEW-02 修复 2：checker 必须把 fixtureState.path 传给执行器。
  let receivedPath = null
  const probe = fakeRunner((input) => {
    receivedPath = input.path
    return { content: [{ type: 'text', text: 'x' }], details: { scanComplete: false, scanTruncated: true, matchLimitReached: true, returnedCount: 200, totalMatches: null } }
  })
  await checkSearchFieldDistinction({ task, fixtureState: { path: 'fixture-directory' }, runTool: probe })
  assert.equal(receivedPath, 'fixture-directory')

  // 修复 3：完整扫描+返回裁剪（totalMatches 已知且 > returnedCount）必须通过。
  const completeScanCapped = fakeRunner(() => ({
    content: [{ type: 'text', text: 'x' }],
    details: { scanComplete: true, scanTruncated: false, matchLimitReached: true, returnedCount: 200, totalMatches: 300 },
  }))
  assert.equal((await checkSearchFieldDistinction({ task, fixtureState: { path: 'd' }, runTool: completeScanCapped })).passed, true)

  // 修复 3：扫描预算耗尽也合法。
  const budgetExhausted = fakeRunner(() => ({
    content: [{ type: 'text', text: 'x' }],
    details: { scanComplete: false, scanTruncated: true, matchLimitReached: true, returnedCount: 200, totalMatches: null },
  }))
  assert.equal((await checkSearchFieldDistinction({ task, fixtureState: { path: 'd' }, runTool: budgetExhausted })).passed, true)

  const missing = fakeRunner(() => ({ content: [{ type: 'text', text: 'x' }], details: { count: 200 } }))
  assert.equal((await checkSearchFieldDistinction({ task, fixtureState: { path: 'd' }, runTool: missing })).passed, false)

  // 修复 3：自相矛盾（声称完成且未截断、命中上限、总数未知）必须失败。
  const lying = fakeRunner(() => ({
    content: [{ type: 'text', text: 'x' }],
    details: { scanComplete: true, scanTruncated: false, matchLimitReached: true, returnedCount: 200, totalMatches: null },
  }))
  assert.equal((await checkSearchFieldDistinction({ task, fixtureState: { path: 'd' }, runTool: lying })).passed, false)
})

// ---------------------------------------------------------------------------
// O-REVIEW-03 门禁 1：证据身份绑定。O 证明错 case / 全零 productSha / 错契约 /
// 1970 时间 / 空 checks 的旧记录会被 CLI 当成当前产品 executed_all_pass。
// 以下固定该攻击面：每一类冒认都必须被拒绝，且合法记录必须通过。
// ---------------------------------------------------------------------------

const ET01 = TASKS.find((task) => task.id === 'E-T-01')
const GOOD_PRODUCT = { commit: 'f1c23e7992af41b0c9b867532f83493036ce1d56', branch: 'harness/integration', dirty: true }
const GOOD_HARNESS_SHA = 'a'.repeat(64)

// 一份逐字段合法的记录：身份四项与本次运行完全一致、时间新鲜、检查齐备且一致。
function goodRecord(overrides = {}) {
  const checks = ET01.requiredChecks.map((id) => ({ id, passed: true, detail: 'ok' }))
  const base = {
    case: 'E-T-01',
    harness: 'harness-v1.2/rust-host',
    executionId: 'harness-self-test',
    contractVersion: 'CONTRACTS v1.2',
    productSha: GOOD_PRODUCT.commit,
    productRoot: 'D:/example/fox',
    harnessSourceSha256: GOOD_HARNESS_SHA,
    problemIds: ['A01'],
    tool: 'run_command',
    generatedAtMs: Date.now(),
    verdictScope: 'the product base this cargo test compiled against',
    productFiles: MEASURED_PRODUCT_FILES.map((path) => ({ path, sha256: 'b'.repeat(64) })),
    overall: 'passed',
    checks,
  }
  const record = { ...base, ...overrides }
  record.executionStamp = executionStampOf({
    productSha: record.productSha,
    harnessSourceSha256: record.harnessSourceSha256,
    contractVersion: record.contractVersion,
    caseId: record.case,
  })
  return record
}

function verdict(record, options = {}) {
  return validateEvidence({
    record,
    task: ET01,
    product: GOOD_PRODUCT,
    harnessSourceSha256: GOOD_HARNESS_SHA,
    contractVersion: 'CONTRACTS v1.2',
    productRoot: 'D:/example/fox',
    measuredFiles: MEASURED_PRODUCT_FILES.map((path) => ({ path, sha256: 'b'.repeat(64) })),
    expectedExecutionId: 'harness-self-test',
    ...options,
  })
}

test('evidence identity: a fully bound record is accepted', async () => {
  const { valid, rejections } = await verdict(goodRecord())
  assert.equal(valid, true, JSON.stringify(rejections))
})

test('evidence identity: malformed fields and duplicate manifest cannot bypass validation', async () => {
  const mutations = [
    r => { r.case = 42 }, r => { r.productSha = 42 },
    r => { r.contractVersion = 42 }, r => { r.harnessSourceSha256 = 42 },
    r => { r.productFiles = {} }, r => { r.productFiles = r.productFiles.map(() => r.productFiles[0]) },
    r => { r.checks.forEach(row => { row.passed = 'true' }) },
    r => { r.generatedAtMs = String(r.generatedAtMs) }, r => { r.executionId = 'previous-run' },
  ]
  for (const mutate of mutations) {
    const record = goodRecord(); mutate(record)
    assert.equal((await verdict(record)).valid, false, JSON.stringify(record))
  }
  assert.equal((await verdict(goodRecord(), { expectedExecutionId: undefined })).valid, false)
})

test('evidence identity: wrong case is rejected', async () => {
  const { valid, rejections } = await verdict(goodRecord({ case: 'WRONG-CASE' }))
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('case 不匹配')), JSON.stringify(rejections))
})

test('evidence identity: all-zero productSha is rejected', async () => {
  const { valid, rejections } = await verdict(goodRecord({ productSha: '0'.repeat(40) }))
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('productSha 全零')), JSON.stringify(rejections))
})

test('evidence identity: wrong product commit is rejected', async () => {
  const { valid, rejections } = await verdict(goodRecord({ productSha: 'e7a80dc000000000000000000000000000000000' }))
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('productSha 不匹配')), JSON.stringify(rejections))
})

test('evidence identity: wrong contract version is rejected', async () => {
  const { valid, rejections } = await verdict(goodRecord({ contractVersion: 'wrong' }))
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('contractVersion 不匹配')), JSON.stringify(rejections))
})

test('evidence identity: a 1970 timestamp is rejected', async () => {
  const { valid, rejections } = await verdict(goodRecord({ generatedAtMs: 1 }))
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('generatedAtMs 非法')), JSON.stringify(rejections))
})

test('evidence identity: an empty check set never passes', async () => {
  const record = goodRecord({ checks: [] })
  const { valid, rejections } = await verdict(record)
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('checks 为空')), JSON.stringify(rejections))
  // And the checker itself must not pass it on either.
  const outcome = checkCommandStdoutOnlyFailure({ task: ET01, evidence: { rustHost: record } })
  assert.equal(outcome.passed, false)
})

test('evidence identity: a missing required check is rejected', async () => {
  const checks = ET01.requiredChecks.slice(1).map((id) => ({ id, passed: true, detail: 'ok' }))
  const { valid, rejections } = await verdict(goodRecord({ checks }))
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('缺少必需检查')), JSON.stringify(rejections))
})

test('evidence identity: overall=passed with failed checks is rejected', async () => {
  const checks = ET01.requiredChecks.map((id) => ({ id, passed: id === 'et01:stdout-diagnostics-preserved', detail: 'dropped' }))
  const { valid, rejections } = await verdict(goodRecord({ checks, overall: 'passed' }))
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('矛盾')), JSON.stringify(rejections))
})

test('evidence identity: a stale record outside the freshness window is rejected', async () => {
  const { valid, rejections } = await verdict(goodRecord({ generatedAtMs: Date.now() - 1000 }), { maxAgeMs: 500 })
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('记录过期')), JSON.stringify(rejections))
})

test('evidence identity: an execution stamp from another product is rejected', async () => {
  const record = goodRecord()
  record.executionStamp = executionStampOf({
    productSha: '9999999999999999999999999999999999999999',
    harnessSourceSha256: GOOD_HARNESS_SHA,
    contractVersion: 'CONTRACTS v1.2',
    caseId: 'E-T-01',
  })
  const { valid, rejections } = await verdict(record)
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('executionStamp')), JSON.stringify(rejections))
})

test('evidence identity: changed measured product bytes are rejected (dirty hash binding)', async () => {
  const measuredFiles = MEASURED_PRODUCT_FILES.map((path, index) => ({ path, sha256: index % 2 ? 'b'.repeat(64) : 'c'.repeat(64) }))
  const { valid, rejections } = await verdict(goodRecord(), { measuredFiles })
  assert.equal(valid, false)
  assert.ok(rejections.some((reason) => reason.includes('productFiles 内容不一致')), JSON.stringify(rejections))
})

test('evidence identity: missing identity fields are rejected', async () => {
  const record = goodRecord()
  delete record.productSha
  delete record.executionStamp
  delete record.harnessSourceSha256
  const { valid, rejections } = await verdict(record)
  assert.equal(valid, false)
  assert.ok(rejections.length >= 3, JSON.stringify(rejections))
})

test('evidence identity: checker does not pass a rejected record shape', async () => {
  // 纵深防御：即便上游漏校验，checker 自身也拒绝 0 检查与 overall 不一致。
  const empty = checkCommandStdoutOnlyFailure({ task: ET01, evidence: { rustHost: goodRecord({ checks: [] }) } })
  assert.equal(empty.passed, false)
  const contradictory = goodRecord({ checks: ET01.requiredChecks.map((id) => ({ id, passed: false, detail: 'x' })), overall: 'passed' })
  contradictory.executionStamp = 'whatever'
  const outcome = checkCommandStdoutOnlyFailure({ task: ET01, evidence: { rustHost: contradictory } })
  assert.equal(outcome.passed, false)
})
