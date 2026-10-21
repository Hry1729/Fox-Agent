import { describe, expect, test } from 'bun:test'
import {
  deriveRunCancellations,
  deriveRunFailures,
  runFailureDetailText,
  type RunFailureInput,
} from '../src/features/conversations/model/run-failure'

function persistedRun(overrides: Partial<{
  id: string
  status: string
  errorCode: string | null
  errorMessage: string | null
}> = {}) {
  return {
    id: 'run-1',
    status: 'failed',
    errorCode: null,
    errorMessage: null,
    ...overrides,
  }
}

describe('deriveRunFailures', () => {
  test('derives a failure from a persisted failed run with code and message', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({
        id: 'run-failed',
        status: 'failed',
        errorCode: 'kernel.context_compaction_insufficient',
        errorMessage: '上下文精简空间不足',
      })],
      events: [],
    })

    expect([...failures.keys()]).toEqual(['run-failed'])
    expect(failures.get('run-failed')).toEqual({
      kind: 'failed',
      code: 'kernel.context_compaction_insufficient',
      message: '上下文精简空间不足',
    })
  })

  test('derives a failure even when every assistant message of the run is completed', () => {
    // The reducer/projection write `completed` for each assistant message, so the
    // failure must not depend on message.status. The derivation input has no
    // messages at all; they are included here only to document the scenario.
    const assistantMessages = Array.from({ length: 13 }, (_, index) => ({
      id: `assistant-${index}`,
      runId: 'run-1',
      role: 'assistant' as const,
      status: 'completed',
    }))

    const input = {
      persistedRuns: [persistedRun({
        id: 'run-1',
        status: 'failed',
        errorCode: 'kernel.context_compaction_insufficient',
        errorMessage: '上下文精简空间不足',
      })],
      events: [],
      messages: assistantMessages,
    } as RunFailureInput

    const failures = deriveRunFailures(input)
    expect(assistantMessages.every((message) => message.status === 'completed')).toBe(true)
    expect(failures.size).toBe(1)
    expect(failures.get('run-1')?.kind).toBe('failed')
  })

  test('treats a cancelled run as cancelled, not as a failure', () => {
    const input: RunFailureInput = {
      persistedRuns: [persistedRun({ id: 'run-cancelled', status: 'cancelled', errorCode: null, errorMessage: null })],
      events: [],
    }

    expect(deriveRunFailures(input).size).toBe(0)
    expect([...deriveRunCancellations(input)]).toEqual(['run-cancelled'])
  })

  test('derives from persisted state alone when the terminal event is absent from events', () => {
    // Covers re-entering a conversation, app restart, and a terminal event that
    // fell outside the loaded pagination window.
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({
        id: 'run-paginated',
        status: 'interrupted',
        errorCode: 'runtime.interrupted',
        errorMessage: '运行被中断',
      })],
      events: [],
    })

    expect(failures.get('run-paginated')).toEqual({
      kind: 'interrupted',
      code: 'runtime.interrupted',
      message: '运行被中断',
    })
  })

  test('derives from events alone when no persisted run state is available', () => {
    const failures = deriveRunFailures({
      persistedRuns: [],
      events: [{
        runId: 'run-live',
        eventType: 'run.failed',
        event: { type: 'run.failed', code: 'provider.timeout', message: '模型响应超时' },
      }],
    })

    expect(failures.get('run-live')).toEqual({
      kind: 'failed',
      code: 'provider.timeout',
      message: '模型响应超时',
    })
  })

  test('keeps a single record per run id', () => {
    const failures = deriveRunFailures({
      persistedRuns: [
        persistedRun({ id: 'run-1', status: 'failed', errorCode: 'e1', errorMessage: '第一次' }),
        persistedRun({ id: 'run-1', status: 'failed', errorCode: 'e2', errorMessage: '第二次' }),
      ],
      events: [
        { runId: 'run-1', eventType: 'run.failed', event: { type: 'run.failed', code: 'evt', message: '事件' } },
        { runId: 'run-1', eventType: 'run.failed', event: { type: 'run.failed', code: 'evt2', message: '事件2' } },
      ],
    })

    expect(failures.size).toBe(1)
  })

  test('lets the persisted terminal state override the event stream', () => {
    const input: RunFailureInput = {
      persistedRuns: [persistedRun({ id: 'run-1', status: 'failed', errorCode: 'persisted', errorMessage: '持久化' })],
      events: [{ runId: 'run-1', eventType: 'run.failed', event: { type: 'run.failed', code: 'event', message: '事件' } }],
    }

    expect(deriveRunFailures(input).get('run-1')).toEqual({
      kind: 'failed',
      code: 'persisted',
      message: '持久化',
    })
  })

  test('falls back to a default message and code when the source omits them', () => {
    const failures = deriveRunFailures({
      events: [{ runId: 'run-1', eventType: 'run.interrupted', event: { type: 'run.interrupted' } }],
    })

    expect(failures.get('run-1')).toEqual({
      kind: 'interrupted',
      code: 'unknown',
      message: '运行中断',
    })
  })
})

describe('run failure stderr diagnostic', () => {
  const tail = 'mock: DEEPSEEK_API_KEY=[redacted]\nmock: Authorization: Bearer [redacted]\nmock: Error: Connection error (last line)'

  test('a failed run event carries the redacted stderr tail into the failure', () => {
    const failures = deriveRunFailures({
      persistedRuns: [],
      events: [{
        runId: 'run-ext',
        eventType: 'run.failed',
        event: {
          type: 'run.failed',
          code: 'agent_backend_prompt_failed',
          message: 'prompt failed: Internal error',
          diagnostic: { kind: 'stderr_tail', source: 'prompt_failed', redacted: true, maxBytes: 4096, stderrTail: tail },
        },
      }],
    })
    const failure = failures.get('run-ext')!
    expect(failure.stderrTail).toBe(tail)
    expect(failure.stderrTailSource).toBe('prompt_failed')
    const text = runFailureDetailText(failure)
    expect(text.startsWith('agent_backend_prompt_failed: prompt failed: Internal error')).toBe(true)
    expect(text).toContain('后端 stderr 尾部（已脱敏，最多 4 KiB；本轮请求失败）：')
    expect(text.endsWith('Error: Connection error (last line)')).toBe(true)
    expect(text).not.toContain('sk-')
  })

  test('the persisted run row wins for code and message but keeps the tail from the event', () => {
    const failures = deriveRunFailures({
      persistedRuns: [persistedRun({ id: 'run-ext', status: 'failed', errorCode: 'agent_backend_exited', errorMessage: '外部后端进程意外退出' })],
      events: [{
        runId: 'run-ext',
        eventType: 'run.failed',
        event: { type: 'run.failed', code: 'agent_backend_exited', message: 'x', diagnostic: { source: 'backend_exited', stderrTail: tail } },
      }],
    })
    expect(failures.get('run-ext')).toEqual({
      kind: 'failed',
      code: 'agent_backend_exited',
      message: '外部后端进程意外退出',
      stderrTail: tail,
      stderrTailSource: 'backend_exited',
    })
  })

  test('without a diagnostic the failure and its detail text are unchanged', () => {
    const failures = deriveRunFailures({
      persistedRuns: [],
      events: [{ runId: 'run-plain', eventType: 'run.failed', event: { type: 'run.failed', code: 'kernel.x', message: 'boom', diagnostic: { stderrTail: '   ' } } }],
    })
    const failure = failures.get('run-plain')!
    expect(failure).toEqual({ kind: 'failed', code: 'kernel.x', message: 'boom' })
    expect('stderrTail' in failure).toBe(false)
    expect(runFailureDetailText(failure)).toBe('kernel.x: boom')
  })
})
