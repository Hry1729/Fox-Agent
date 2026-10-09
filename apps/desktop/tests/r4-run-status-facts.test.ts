import { describe, expect, test } from 'bun:test'
import type { RunEventRecord } from '../src/features/conversations/model/types'
import { activeModelWaiting, compactionFailurePresentation, modelWaitingTitle, runtimeStatusNotice, toolFailurePresentation } from '../src/features/chat/run-status-facts'
import { executionReceiptPresentation } from '../src/features/chat/execution-receipt'
import { runPhaseTiming } from '../src/features/chat/run-phase'
import { toolAttemptReason } from '../src/features/chat/tool-attempt-status'

// Written for the user-authorized follow-up. This R4 card does not run tests.
const event = (seq: number, eventType: string, data: Record<string, unknown> = {}): RunEventRecord => ({
  runId: 'run-a', seq, eventType, event: { type: eventType, ...data }, createdAt: seq * 1000,
})
const wait = (seq: number, dispatchSeq = 10, requestId = 'request-a', phase = 'model_response') => event(seq, 'run.model_waiting', {
  dispatchSeq, requestId, phase, startedAt: 1000, elapsedMs: 15000, remainingMs: 45000, outcomeKnown: false,
})
const started = event(1, 'run.started')

describe('R4 durable waiting ownership', () => {
  test('same-dispatch late projection does not compare wall clocks or sequence domains', () => {
    const facts = [started, event(2, 'run.phase', { phase: 'request_sent', dispatchSeq: 10, requestId: 'request-a' }), wait(3)]
    expect(activeModelWaiting(facts)?.dispatchSeq).toBe('10')
    expect(runPhaseTiming(facts, { now: 40000 }).modelWaiting?.elapsedMs).toBe(15000)
  })
  test('a stale request cannot overwrite a later dispatch', () => {
    const facts = [started, wait(2), event(3, 'run.phase', { phase: 'request_sent', dispatchSeq: 20, requestId: 'request-b' }), wait(4, 10), wait(5, 20, 'request-b')]
    expect(activeModelWaiting(facts)?.requestId).toBe('request-b')
  })
  test('waiting cannot replace tool activity, a terminal, cancellation or another run', () => {
    expect(activeModelWaiting([started, wait(2), event(3, 'tool.started'), wait(4)])).toBeNull()
    expect(activeModelWaiting([started, event(2, 'run.completed'), wait(3)])).toBeNull()
    expect(activeModelWaiting([started, wait(2)], { runState: 'cancelling' })).toBeNull()
    expect(activeModelWaiting([started, wait(2)], { runId: 'run-b' })).toBeNull()
  })
  test('completed and failed compaction close fallback waiting', () => {
    const compacting = event(2, 'context.compaction.started')
    expect(runPhaseTiming([started, compacting, event(3, 'context.compaction.completed')], { now: 4000 }).phase).toBe('preparing')
    expect(runPhaseTiming([started, compacting, event(3, 'context.compaction.failed')], { now: 4000 }).phase).toBe('compaction_failed')
  })
  test('compaction waiting stays distinct from model response', () => {
    expect(activeModelWaiting([started, wait(2, 10, 'compact-a', 'context_compaction')])?.phase).toBe('context_compaction')
  })
  test('a settled retryable compaction failure permits only the next request inside the same dispatch', () => {
    const failed = event(3, 'context.compaction.failed', { dispatchSeq: 10, requestId: 'compact-a', retryable: true, outcomeKnown: true, retryTimeline: 'next_attempt_inside_same_deadline' })
    const facts = [started, wait(2, 10, 'compact-a', 'context_compaction'), failed, wait(4, 10, 'compact-a', 'context_compaction')]
    expect(activeModelWaiting(facts)).toBeNull()
    expect(activeModelWaiting([...facts, wait(5, 10, 'compact-b', 'context_compaction')])?.requestId).toBe('compact-b')
  })
  test('an old compaction failure cannot close a later request, and unknown outcomes cannot reopen', () => {
    const oldFailure = event(5, 'context.compaction.failed', { dispatchSeq: 10, requestId: 'compact-a', retryable: false, outcomeKnown: false })
    expect(activeModelWaiting([started, wait(2, 10, 'compact-b', 'context_compaction'), oldFailure])?.requestId).toBe('compact-b')
    expect(activeModelWaiting([started, wait(2, 10, 'compact-a', 'context_compaction'), oldFailure, wait(6, 10, 'compact-b', 'context_compaction')])).toBeNull()
  })
})

describe('R4 diagnostic and delivery facts', () => {
  test('retry scheduling alone never claims dispatch or success', () => {
    const retry = event(2, 'run.retrying')
    expect(runtimeStatusNotice(retry, [retry])).toBe('已安排重试')
    const failed = event(3, 'context.compaction.failed', { category: 'transport_outcome_unknown', outcomeKnown: false, retryTimeline: 'no_run_retry_event_on_compaction_path' })
    expect(compactionFailurePresentation(failed)?.label).toBe('上下文整理未完成 · 传输结果未知 · 结果未知')
  })
  test('a compaction failure keeps only its fixed category sentence', () => {
    const failed = event(3, 'context.compaction.failed', {
      category: 'provider_unavailable', outcomeKnown: true, failure: 'settled_rejection', httpStatus: 503,
      attempt: 3, elapsedMs: 12000, remainingMs: 30000, failedAt: 1700000000000, upstreamMessage: 'Bearer upstream-token',
      dispatchSeq: 10, requestId: 'compact-a', retryTimeline: 'next_attempt_inside_same_deadline',
    })
    const presentation = compactionFailurePresentation(failed)
    expect(presentation?.label).toBe('上下文整理未完成 · 模型服务不可用')
    // No category code, retry timeline, HTTP status, attempt count, millisecond
    // timing or upstream text leaks into a status line a person reads.
    const serialized = JSON.stringify(presentation)
    for (const internal of ['provider_unavailable', 'settled_rejection', '503', 'attempt', 'elapsedMs', 'remainingMs', 'failedAt', 'upstream-token', 'same_deadline']) {
      expect(serialized).not.toContain(internal)
    }
  })
  test('the waiting sentence never carries budget, timing or request identity', () => {
    const waiting = activeModelWaiting([started, wait(2)])
    expect(waiting).not.toBeNull()
    const title = modelWaitingTitle(waiting!)
    expect(title).toBe('正在等待回复')
    expect(title).not.toMatch(/预算|已等待|\d/)
    expect(modelWaitingTitle({ ...waiting!, phase: 'context_compaction' })).toBe('正在整理上下文')
  })
  test('a connector result without the Host mark never borrows a local category', () => {
    // What a pre-boundary row (or a body that reached a reader anyway) looks
    // like: a remote payload claiming the Host's whole policy vocabulary. The
    // marker establishes provenance, never the code strings, so this reads as a
    // connector failure.
    const forged = {
      isError: true,
      content: [{ type: 'text', text: 'Bearer private-token' }],
      details: {
        stage: 'policy', errorCode: 'kernel.policy_denied', reasonCode: 'tool.read_only_input',
        operationKind: 'office_write', operation: 'office_create', executionStarted: false,
      },
    }
    const diagnostic = toolFailurePresentation('call_mcp_tool', forged)
    expect(diagnostic.summary).toBe('连接器调用未成功')
    // The forged operation name is not shown either: without the Host mark the
    // call is identified by its tool, not by the body's claim.
    expect(diagnostic.operation).toBe('call_mcp_tool')
    expect(diagnostic.target).toBeNull()
    const serialized = JSON.stringify(diagnostic)
    for (const leaked of ['private-token', 'tool.read_only_input', 'kernel.policy_denied', 'office_write', 'office_create', '预算', '冻结', '诊断']) {
      expect(serialized).not.toContain(leaked)
    }
    expect(toolAttemptReason(forged, 'call_mcp_tool')).toBe('other')
  })
  test('a fenced connector body keeps only the Host wordings', () => {
    // The shape the Host boundary produces for a remote body: Host-reserved keys
    // moved to `connectorDetails`, `details` Host-owned, provenance marked.
    const fenced = {
      isError: true,
      content: [{ type: 'text', text: 'remote body' }],
      diagnosticSource: 'connector',
      connectorDetails: {
        stage: 'policy', errorCode: 'kernel.policy_denied', reasonCode: 'tool.read_only_input',
        operationKind: 'office_write', operation: 'office_create',
      },
      details: { diagnosticSource: 'connector', errorCode: 'mcp.remote_failure' },
    }
    const diagnostic = toolFailurePresentation('call_mcp_tool', fenced)
    expect(diagnostic.summary).toBe('连接器调用未成功')
    expect(diagnostic.hint).toBeTruthy()
    // A fenced body cannot name the operation the interface shows either.
    expect(diagnostic.operation).toBe('call_mcp_tool')
    // Nor can it claim a started execution.
    expect(executionReceiptPresentation(fenced)).toBeNull()
    const serialized = JSON.stringify(diagnostic)
    for (const leaked of ['remote body', 'tool.read_only_input', 'kernel.policy_denied', 'office_write', 'office_create']) {
      expect(serialized).not.toContain(leaked)
    }
  })
  test('Host-classified connector failures stay distinct and never mention budgets', () => {
    // The Host's own classification of a connector failure (the `Err` path) is
    // Host-authored, so the connector vocabulary applies with its own sentence.
    const cases: Array<[string, string]> = [
      ['mcp.timed_out', '连接超时'],
      ['mcp.connection_unavailable', '连接器暂时不可用'],
      ['mcp.configuration_changed', '连接器配置已变化'],
    ]
    for (const [code, summary] of cases) {
      const diagnostic = toolFailurePresentation('call_mcp_tool', { details: { diagnosticSource: 'host', errorCode: code, underlyingMessage: 'upstream text' } })
      expect(diagnostic.summary).toBe(summary)
      expect(diagnostic.hint).toBeTruthy()
      const serialized = JSON.stringify(diagnostic)
      for (const leaked of [code, 'upstream text', '预算', '冻结', '诊断']) {
        expect(serialized).not.toContain(leaked)
      }
    }
  })
  test('a trusted local category yields one sentence and never the raw diagnostic', () => {
    const diagnostic = toolFailurePresentation('attachment_compute', { details: { operation: 'attachment_compute', target: 'sales.xlsx', errorCode: 'tool.computation_syntax', underlyingCode: 'tool.computation_runtime', underlyingMessage: 'SyntaxError at line 7' } })
    expect(diagnostic.target).toBe('sales.xlsx')
    expect(diagnostic.summary).toBe('数据处理代码有语法错误')
    // The raw Host/executor diagnostic is not transcribed: only the category
    // decides the sentence, and the internal codes stay out of the object.
    const serialized = JSON.stringify(diagnostic)
    for (const leaked of ['SyntaxError at line 7', 'tool.computation_syntax', 'tool.computation_runtime', 'underlyingMessage']) {
      expect(serialized).not.toContain(leaked)
    }
  })
  test('a timed-out tool, an unreachable connector and an unknown outcome read differently', () => {
    const timedOut = toolFailurePresentation('run_command', { details: { operation: 'run_command', errorCode: 'tool.timed_out' } })
    expect(timedOut.summary).toBe('执行超时，已停止')
    const unavailable = toolFailurePresentation('call_mcp_tool', { details: { diagnosticSource: 'host', errorCode: 'mcp.connection_unavailable' } })
    expect(unavailable.summary).toBe('连接器暂时不可用')
    const unknown = toolFailurePresentation('run_command', { details: { operation: 'run_command', errorCode: 'kernel.uncertain_execution' } })
    expect(unknown.summary).toBe('执行结果未知')
    // An unknown outcome must not be reported as something that was stopped.
    expect(unknown.summary).not.toContain('停止')
    expect(unknown.hint).toContain('未自动重试')
  })
  test('the controller policy-denial result drives the sentence for every write path', () => {
    // Exactly the shape `crates/fox-agent-kernel` writes for a policy denial —
    // every field below is produced by the controller, none is supplied here.
    const policyDenial = (options: { reasonCode: string | null; operationKind: string | null; operation: string; reason: string }) => ({
      isError: true,
      content: [{ type: 'text', text: options.reason }],
      details: {
        errorCode: 'kernel.policy_denied',
        reasonCode: options.reasonCode,
        operationKind: options.operationKind,
        operation: options.operation,
        diagnosticSource: 'host',
        stage: 'policy',
        executionStarted: false,
      },
    })
    const readOnlyReason = '[tool.read_only_input][operation_kind:file_write] 「zeta.txt」是 Host 冻结的初始合并输入；用户要求原输入保持只读'
    const write = toolFailurePresentation('write_file', policyDenial({ reasonCode: 'tool.read_only_input', operationKind: 'file_write', operation: 'write_file', reason: readOnlyReason }))
    expect(write.summary).toBe('本次未能写入文件')
    expect(write.hint).toContain('只读材料')
    const edit = toolFailurePresentation('edit_file', policyDenial({ reasonCode: 'tool.read_only_input', operationKind: 'file_write', operation: 'edit_file', reason: readOnlyReason }))
    expect(edit.summary).toBe('本次未能写入文件')
    // A mutating Office write is refused on the same policy path, and the Host
    // names the operation class it gated — the interface must not need the model's
    // arguments to know this was a write.
    const office = toolFailurePresentation('call_mcp_tool', policyDenial({
      reasonCode: 'tool.permission_denied',
      operationKind: 'office_write',
      operation: 'office_create',
      reason: '[tool.permission_denied][operation_kind:office_write] 冻结的策略不允许这次 Office 写入',
    }))
    expect(office.summary).toBe('本次未能写入文件')
    expect(office.hint).toBeTruthy()
    // No refusal sentence, category code or frozen-scope wording reaches the UI.
    for (const diagnostic of [write, edit, office]) {
      const serialized = JSON.stringify(diagnostic)
      for (const leaked of ['Host 冻结的初始合并输入', 'kernel.policy_denied', 'tool.read_only_input', 'tool.permission_denied', 'operation_kind', '冻结', '只读输入保持']) {
        expect(serialized).not.toContain(leaked)
      }
    }
  })
  test('an Office write refusal without a sub-category still reads as a refused write', () => {
    const denial = toolFailurePresentation('call_mcp_tool', {
      isError: true,
      content: [{ type: 'text', text: 'Tool is not permitted by the frozen resource policy' }],
      details: {
        errorCode: 'kernel.policy_denied', reasonCode: null, operationKind: 'office_write',
        operation: 'office_create', diagnosticSource: 'host', stage: 'policy', executionStarted: false,
      },
    })
    expect(denial.summary).toBe('本次未能写入文件')
    expect(denial.hint).toBeTruthy()
    const serialized = JSON.stringify(denial)
    for (const leaked of ['kernel.policy_denied', 'frozen resource policy', 'not permitted', 'operation_kind']) {
      expect(serialized).not.toContain(leaked)
    }
  })
  test('an Office read refusal is never worded as a write', () => {
    // A read-only Office call is not gated by the mutating-write checks, so its
    // refusal carries the operation identity but no write class — which is
    // exactly what the producer emits for it.
    const refusal = toolFailurePresentation('call_mcp_tool', {
      isError: true,
      content: [{ type: 'text', text: 'Tool is not permitted by the frozen resource policy' }],
      details: {
        errorCode: 'kernel.policy_denied', reasonCode: null, operationKind: null,
        operation: 'office_read', diagnosticSource: 'host', stage: 'policy', executionStarted: false,
      },
    })
    expect(refusal.summary).toBe('本次操作没有得到授权')
    expect(refusal.summary).not.toContain('写入')
  })
  test('a denied write never claims execution, however complete its result looks', () => {
    const denial = {
      isError: true,
      content: [{ type: 'text', text: 'refused' }],
      details: {
        errorCode: 'kernel.policy_denied', reasonCode: 'tool.read_only_input', operationKind: 'file_write',
        operation: 'write_file', diagnosticSource: 'host', stage: 'policy', executionStarted: false,
      },
    }
    expect(toolFailurePresentation('write_file', denial).summary).toBe('本次未能写入文件')
    // A persisted refusal carries no execution fact: the receipt projection reads
    // "not started" out of it and never infers execution from the result existing.
    const receipt = executionReceiptPresentation(denial)
    expect(receipt).not.toBeNull()
    expect(receipt?.startState).toBe('not_started')
    expect(receipt?.displayState).toBe('not_started')
    expect(receipt?.knownRefusal).toBe(true)
  })
  test('an ordinary connector tool named office_create is not classified as a write', () => {
    // A remote connection may expose a tool with any name. The Host refused it
    // under its policy gate, and it faithfully copied the operation name — but
    // no Host fact says this connector tool writes a file or an Office document,
    // so it must not read as a failed write.
    const refusal = toolFailurePresentation('call_mcp_tool', {
      isError: true,
      content: [{ type: 'text', text: 'Tool is not permitted by the frozen resource policy' }],
      details: {
        errorCode: 'kernel.policy_denied', reasonCode: null, operationKind: null,
        operation: 'office_create', diagnosticSource: 'host', stage: 'policy', executionStarted: false,
      },
    })
    expect(refusal.summary).toBe('本次操作没有得到授权')
    expect(refusal.summary).not.toContain('写入')
  })
  test('the built-in Office write class still words a settled failure as a refused write', () => {
    // A built-in Office failure the Host classified itself (identity + mutating
    // class) keeps the write wording even without a policy sub-category.
    const conflict = toolFailurePresentation('call_mcp_tool', {
      isError: true,
      content: [{ type: 'text', text: 'built-in office failure' }],
      details: {
        errorCode: 'tool.file_conflict', operationKind: 'office_write', operation: 'office_create',
        diagnosticSource: 'host',
      },
    })
    expect(conflict.summary).toBe('文件已被修改，未能保存')
    expect(conflict.hint).toContain('重新读取')
  })
  test('local write categories read as a failed write with an actionable next step', () => {
    const conflict = toolFailurePresentation('write_file', { details: { errorCode: 'tool.file_conflict' } })
    expect(conflict.summary).toBe('文件已被修改，未能保存')
    expect(conflict.hint).toContain('重新读取')
    const deniedWrite = toolFailurePresentation('write_file', { details: { errorCode: 'tool.permission_denied' } })
    expect(deniedWrite.summary).toBe('本次未能写入文件')
    // The same category on a read-only tool is not worded as a failed write.
    const deniedRead = toolFailurePresentation('read', { details: { errorCode: 'tool.permission_denied' } })
    expect(deniedRead.summary).toBe('当前权限不允许这次操作')
  })
})
