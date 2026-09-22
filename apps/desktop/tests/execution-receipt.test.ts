import { describe, expect, test } from 'bun:test'
import { executionReceiptPresentation } from '../src/features/chat/execution-receipt'

function result(executionReceipt: Record<string, unknown>, code?: string) {
  return {
    details: {
      executionReceipt,
      ...(code ? { error: { code } } : {}),
    },
  }
}

describe('executionReceiptPresentation', () => {
  test('keeps a known process refusal separate from uncertain execution', () => {
    const presentation = executionReceiptPresentation(result({
      kind: 'process', stage: 'admitted', executionStarted: 'false',
      controlPlane: 'prepared', externalEffect: 'none',
      reasonCode: 'policy_denied', codeAlias: null, allowReplay: false,
    }))

    expect(presentation).toMatchObject({
      displayState: 'not_started', statusLabel: '进程未启动 · 已拒绝',
      code: 'policy_denied', knownRefusal: true, uncertain: false,
    })
  })

  test('presents a completed read or manage call without claiming a process failure', () => {
    const presentation = executionReceiptPresentation(result({
      kind: 'process', stage: 'admitted', executionStarted: 'false',
      controlPlane: 'committed', externalEffect: 'none',
      reasonCode: null, codeAlias: null, allowReplay: false,
    }))

    expect(presentation).toMatchObject({
      displayState: 'not_started', startState: 'not_started',
      statusLabel: '调用已完成（未启动外部操作）',
      knownRefusal: false, uncertain: false, requiresAttention: false,
    })
  })

  test('uses file language for file receipts and never claims a process started', () => {
    const presentation = executionReceiptPresentation(result({
      kind: 'file', stage: 'file_not_applied', executionStarted: 'false',
      controlPlane: 'not_applied', externalEffect: 'not_applied',
      reasonCode: 'credential_mismatch', allowReplay: false,
    }))

    expect(presentation?.statusLabel).toBe('文件操作未开始 · 已拒绝')
    expect(presentation?.statusLabel).not.toContain('进程')
    expect(presentation?.kind).toBe('file')
  })

  test('merges the two uncertain process aliases only for the same uncertain fact', () => {
    const presentation = executionReceiptPresentation(result({
      kind: 'process', stage: 'interrupted', executionStarted: 'unknown',
      controlPlane: 'uncertain', externalEffect: 'uncertain',
      reasonCode: 'kernel.uncertain_execution', codeAlias: 'job.interrupted_unknown',
      allowReplay: true,
    }))

    expect(presentation).toMatchObject({
      displayState: 'start_unknown',
      statusLabel: '进程是否启动未知 · 禁止重放',
      code: 'kernel.uncertain_execution', uncertain: true, replayAllowed: false,
    })
    expect(presentation?.rawCodes).toEqual(['kernel.uncertain_execution', 'job.interrupted_unknown'])
  })

  test('does not let an uncertain alias turn a known refusal into unknown', () => {
    const presentation = executionReceiptPresentation(result({
      kind: 'process', stage: 'admitted', executionStarted: 'false',
      controlPlane: 'prepared', externalEffect: 'none',
      reasonCode: 'policy_denied', codeAlias: 'job.interrupted_unknown', allowReplay: false,
    }))

    expect(presentation).toMatchObject({
      displayState: 'not_started', code: 'policy_denied',
      knownRefusal: true, uncertain: false,
    })
  })

  test('distinguishes a confirmed process start whose outcome is unknown', () => {
    const presentation = executionReceiptPresentation(result({
      kind: 'process', stage: 'interrupted', executionStarted: 'true',
      controlPlane: 'uncertain', externalEffect: 'uncertain',
      reasonCode: 'kernel.uncertain_execution', codeAlias: 'job.interrupted_unknown',
      allowReplay: false,
    }))

    expect(presentation).toMatchObject({
      displayState: 'started_outcome_unknown',
      statusLabel: '进程已启动 · 结果未知 · 禁止重放',
      startState: 'started', uncertain: true, replayAllowed: false,
    })
  })

  test('uses file-safe wording for an indeterminate file operation', () => {
    const presentation = executionReceiptPresentation(result({
      kind: 'file', stage: 'file_indeterminate', executionStarted: 'unknown',
      controlPlane: 'uncertain', externalEffect: 'uncertain',
      reasonCode: 'kernel.uncertain_execution', allowReplay: false,
    }))

    expect(presentation?.statusLabel).toBe('文件操作是否开始未知 · 禁止重放')
    expect(presentation?.statusLabel).not.toContain('进程')
  })

  test('supports compatible direct receipt facts with boolean start evidence', () => {
    const presentation = executionReceiptPresentation({
      details: {
        kind: 'process', stage: 'job_created', executionStarted: false,
        controlPlane: 'not_applied', sideEffectState: 'not_applied',
        error: { code: 'sandbox_unavailable' }, allowReplay: false,
      },
    })

    expect(presentation).toMatchObject({
      displayState: 'not_started', statusLabel: '进程未启动 · 已拒绝',
      code: 'sandbox_unavailable', externalEffect: 'not_applied',
    })
  })

  test('treats a missing-receipt fallback as unknown and non-replayable', () => {
    const presentation = executionReceiptPresentation({
      details: {
        executionStarted: 'unknown', controlPlane: 'unknown', sideEffectState: 'unknown',
        error: { code: 'kernel.uncertain_execution' }, allowReplay: false,
      },
    })

    expect(presentation).toMatchObject({
      kind: 'unknown', displayState: 'start_unknown',
      statusLabel: '是否已执行未知 · 禁止重放',
      code: 'kernel.uncertain_execution', uncertain: true, replayAllowed: false,
    })
  })

  test('ignores ordinary tool results without receipt facts', () => {
    expect(executionReceiptPresentation({ details: { stdout: 'ok', exitCode: 0 } })).toBeNull()
    expect(executionReceiptPresentation(null)).toBeNull()
  })
})
