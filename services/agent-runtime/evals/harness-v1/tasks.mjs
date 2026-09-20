// 固定任务定义（harness-v1.2）。输入固定、允许变化范围显式、预期结果来自契约而非模型自评。
// 每个任务给出：问题引用、tier、依赖入口、确定性 fixture 配方、契约要求、checker 名、基线预期。
// rust-host 任务另给 requiredChecks：evidence 必须逐项含这些检查 id，缺一不可
// （O-REVIEW-03 门禁 1：缺必需检查/空 checks 的记录不能冒充当前产品的验收结论）。

export const HARNESS_VERSION = 'harness-v1.2'
export const CONTRACT_VERSION = 'CONTRACTS v1.2'
export const BASE_PRODUCT_SHA = '5cb665c50dd267c9bc45bbf8b9af4cedfd4efa40'

// 确定性 fixture 配方：同名 fixture 在任何运行中都生成同样字节内容。
export const FIXTURES = {
  bigLines: { kind: 'text', lineCount: 5000, linePrefix: 'line-' },
  unicode: { kind: 'text', text: 'A😀Fox' },
  crlf: { kind: 'text', text: 'one\r\ntwo\r\n' },
  manyMatchFiles: { kind: 'tree', files: 300, needle: 'probe-needle', perFile: 1 },
}

// 契约要求是 checker 断言的内容；expectedOutcomeAtBaseline 是基线 5cb665c 的
// 已核实预期，用于把“修复前失败”与“修复后通过”分开记录。
export const TASKS = [
  {
    id: 'E-T-03',
    title: '只读分页往返无损',
    problemIds: ['B02', 'B03'],
    tier: 'tool-contract',
    requires: 'node-executor',
    tool: 'read',
    fixture: 'bigLines',
    input: { limit: 100 },
    allowedVariance: '无；分页重建必须逐字节等于原文件',
    contractRequirement:
      '以固定 limit 循环读取，每页按实际返回的 UTF-16 长度推进 offset，' +
      '直到 truncated===false；重建文本必须等于原文件内容；' +
      '未到末页时 truncated 必须为 true，末页必须为 false。',
    checker: 'checkPaginationRoundTrip',
    expectedOutcomeAtBaseline: 'pass',
  },
  {
    id: 'E-T-03b',
    title: 'UTF-16 边界安全（代理对不被切断）',
    problemIds: ['B03'],
    tier: 'tool-contract',
    requires: 'node-executor',
    tool: 'read',
    fixture: 'unicode',
    input: { offset: 2, limit: 2 },
    allowedVariance: '返回文本可以与请求范围不同，但必须是良构 UTF-16',
    contractRequirement:
      'RD-v1：Unicode 边界安全。落在代理对内部的 offset 不得返回半截代理对；' +
      '返回的每个 code unit 必须是完整字符的一部分。',
    checker: 'checkUnicodeBoundary',
    expectedOutcomeAtBaseline: 'fail',
  },
  {
    id: 'E-T-04a',
    title: '行模式参数成对校验',
    problemIds: ['B03'],
    tier: 'tool-contract',
    requires: 'node-executor',
    tool: 'read',
    fixture: 'crlf',
    inputs: [
      // O-REVIEW-02 修复 4：有效输入正例。合法调用必须成功，否则说明执行器本身坏了
      // （此失败属于执行器故障，不能冒充"校验严格"）。
      { label: 'valid-pair', args: { startLine: 1, lineCount: 1 }, kind: 'valid' },
      { label: 'missing-lineCount', args: { startLine: 1 }, kind: 'illegal' },
      { label: 'negative', args: { startLine: -1, lineCount: 1 }, kind: 'illegal' },
      { label: 'fractional', args: { startLine: 1, lineCount: 1.5 }, kind: 'illegal' },
    ],
    allowedVariance: '拒绝信息可变；非法必须是 reject（抛错或失败结果），不得默默补默认值；有效输入必须成功',
    contractRequirement:
      'RD-v1：startLine 与 lineCount 成对提供，均为正整数；缺失、负数、小数必须拒绝。' +
      '有效成对输入必须正常返回结果（正例），用以证明执行器并非"一律拒绝"。',
    checker: 'checkLineModePairing',
    expectedOutcomeAtBaseline: 'fail',
  },
  {
    id: 'E-T-04b',
    title: '行模式 pageComplete 语义',
    problemIds: ['B02'],
    tier: 'tool-contract',
    requires: 'node-executor',
    tool: 'read',
    fixture: 'crlf',
    input: { startLine: 1, lineCount: 2 },
    limits: { maxReadChars: 2 },
    allowedVariance: '返回长度可变；pageComplete 必须反映实际返回范围',
    contractRequirement:
      'RD-v1：pageComplete 必须反映实际返回范围。当请求的行文本被 maxReadChars/output ' +
      '上限丢弃时，pageComplete 必须为 false 且字段必须存在；不得声称已完成。',
    checker: 'checkLinePageComplete',
    expectedOutcomeAtBaseline: 'fail',
  },
  {
    id: 'E-T-04c',
    title: '搜索扫描/匹配/上限区分',
    problemIds: ['B01', 'B02'],
    tier: 'tool-contract',
    requires: 'node-executor',
    tool: 'grep',
    fixture: 'manyMatchFiles',
    input: { pattern: 'probe-needle' },
    limits: { maxMatches: 200 },
    allowedVariance: '匹配顺序可变；计数字段必须存在且语义正确',
    contractRequirement:
      'RD-v1：搜索详情必须区分 scanComplete（作用域是否穷尽）、scanTruncated（扫描被截断）、' +
      'matchLimitReached（命中上限）、returnedCount（本次返回数）、totalMatches（未知为 null）。' +
      '命中上限时 matchLimitReached 必须为 true。两种情形都合法且必须内部一致：' +
      '（a）完整扫描+返回裁剪：scanComplete=true 且 totalMatches 为已知数字且大于 returnedCount；' +
      '（b）扫描预算耗尽：scanComplete=false（作用域未穷尽）。' +
      '不允许 scanComplete=true、scanTruncated!=true、matchLimitReached=true 且 totalMatches=null 的自相矛盾。',
    checker: 'checkSearchFieldDistinction',
    expectedOutcomeAtBaseline: 'fail',
  },
  {
    id: 'E-T-01',
    title: 'stdout-only 非零退出保留诊断',
    problemIds: ['A01'],
    tier: 'rust-host',
    requires: 'rust-host',
    tool: 'run_command',
    contractRequirement:
      'EX-v1：业务非零退出必须保留 exitCode、stdout/stderr 安全诊断并到达模型；' +
      '持久状态 failed、isError=true、回执失败；不得把非零退出当成功。',
    futureCommand:
      'cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib ' +
      'real_eval_command_stdout_only_non_zero_exit_preserves_diagnostics -- --nocapture',
    // O-REVIEW-03 门禁 1：evidence 必须逐项含这些检查 id。与 real_eval_tests.rs
    // 中 checks.check(...) 的 id 逐字一致，缺一不可。
    requiredChecks: [
      'et01:real-command-executed',
      'et01:non-zero-exit-is-a-failure-receipt',
      'et01:exit-code-reaches-the-model',
      'et01:stdout-diagnostics-preserved',
      'et01:persisted-tool-call-ended-as-a-failure',
      'et01:stored-outcome-is-re-readable-by-reference',
    ],
    futureNote:
      '入口性质：直接 Host/DB/reader 定向 Rust 测试（真实 tool_host 执行器 + 真实持久 ' +
      'ToolCall 行 + tool_result_range 回读），不驱动 Node 模型 worker，也不构成完整 ' +
      'synthetic-provider→Kernel→Office 链；model_receipt 只是该入口的局部投影，' +
      '不能据此声称模型实际收到诊断。' +
      '子进程把唯一 marker 同时写入 stdout 与 side-effect 文件后以 exit 7 退出；' +
      '基线 5cb665c 下 et01:stdout-diagnostics-preserved 记录为失败（非零退出的 stdout 被丢弃），' +
      '其余为通过；A 的集成层合并后该检查转绿，分数变化只能记为产品基线变化，不是评测改进。' +
      '与既有 real_task_evaluation*（synthetic-provider / real-cloud-model）入口并列保留，' +
      '按预算/队列分别验证。',
    checker: 'checkCommandStdoutOnlyFailure',
    expectedOutcomeAtBaseline: 'fail',
  },
  {
    id: 'E-T-02',
    title: '合法文本删除',
    problemIds: ['A03'],
    tier: 'rust-host',
    requires: 'rust-host',
    tool: 'edit_file',
    contractRequirement:
      'EX-v1：newText/content 允许空或纯空白；path/oldText 不允许空。' +
      '用非空 oldText 把内容删除到空必须成功；空 oldText 必须拒绝。',
    futureCommand:
      'cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib ' +
      'real_eval_edit_file_allows_deleting_matched_text_to_empty -- --nocapture',
    requiredChecks: [
      'et02:legal-deletion-is-accepted',
      'et02:deleted-text-is-gone-from-disk',
      'et02:empty-oldtext-is-rejected',
      'et02:rejection-left-the-file-untouched',
      'et02:accepted-edit-persisted-its-outcome',
      'et02:accepted-edit-is-re-readable-by-reference',
    ],
    futureNote:
      '入口性质：直接 Host/DB/reader 定向 Rust 测试，不驱动 Node 模型 worker，' +
      'model_receipt 只是局部投影；与既有 real_task_evaluation* 入口并列保留。' +
      '真实 edit_file 执行器对项目内文件做非空 oldText + 空 newText 的删除，' +
      '独立从磁盘重算结果；空 oldText 为负例并校验拒绝未改写文件。' +
      '基线 5cb665c 下 et02:legal-deletion-is-accepted 与 et02:deleted-text-is-gone-from-disk ' +
      '记录为失败（空 newText 被当作非法参数拒绝），空 oldText 负例通过；' +
      'A 的集成层合并后转绿，分数变化归产品基线，不归评测改进。',
    checker: 'checkLegalDeletion',
    expectedOutcomeAtBaseline: 'fail',
  },
  {
    id: 'E-T-05',
    title: '越权拒绝（项目根外读取）',
    problemIds: ['S01'],
    tier: 'rust-host',
    requires: 'rust-host',
    tool: 'read',
    contractRequirement:
      'RD-v1：越权失败不得伪装为普通跳过。读取冻结项目根之外的路径必须被拒绝，' +
      '且拒绝原因可区分于普通跳过/读取错误；授权根信息不得成为越权依据。',
    futureCommand:
      'cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib ' +
      'real_eval_read_outside_the_frozen_project_root_is_denied_and_re_readable -- --nocapture',
    requiredChecks: [
      'et05:in-root-read-succeeds',
      'et05:in-root-read-persisted-completed',
      'et05:in-root-read-is-re-readable-by-reference',
      'et05:out-of-root-read-is-denied',
      'et05:denial-is-distinguishable-from-an-ordinary-read-error',
      'et05:frozen-root-is-not-a-basis-for-escaping-it',
      'et05:denial-persisted-as-a-failed-tool-call',
      'et05:denial-outcome-is-re-readable-by-reference',
    ],
    futureNote:
      '入口性质：直接 Host/DB/reader 定向 Rust 测试（execute_rust_reader_request 真实 ' +
      'Legacy+Rust 读取缝派），不驱动 Node 模型 worker，model_receipt 只是局部投影；' +
      '与既有 real_task_evaluation* 入口并列保留，按预算/队列分别验证。' +
      '校验根外绝对路径、由冻结根拼接的逃逸路径均被拒，且拒绝原因与根内缺失文件可区分；' +
      '拒绝被持久化为 failed ToolCall 并可按引用回读。' +
      '基线 5cb665c 下全部检查通过（这是基线已具备的能力，不是新修复）。' +
      '作用域裁决在 Rust Host 层（tool_guard/resource_gateway），Node 执行器层不承担。',
    checker: 'checkPermissionDenial',
    expectedOutcomeAtBaseline: 'pass',
  },
]

export function taskById(id) {
  return TASKS.find((task) => task.id === id)
}
