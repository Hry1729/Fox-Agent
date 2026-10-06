import { describe, expect, test } from 'bun:test'
import { toolAttemptReason, toolAttemptState, type ToolAttempt } from '../src/features/chat/tool-attempt-status'

const attempt = (name: string, seq: number, lastSeq: number, isError: boolean, output?: unknown): ToolAttempt => ({
  name, seq, lastSeq, isError, completed: true, output,
})

describe('tool attempt versus task outcome', () => {
  test('a failed call remains pending while the model can adjust, then recovers on a successful retry', () => {
    const failed = attempt('read_file', 2, 3, true, { code: 'invalid_argument' })
    expect(toolAttemptState(failed, [failed], 3, 'running')).toBe('pending')
    const retry = attempt('read_file', 5, 6, false)
    expect(toolAttemptState(failed, [failed, retry], 6, 'running')).toBe('recovered')
    expect(toolAttemptReason(failed.output)).toBe('arguments')
  })

  test('later model progress and terminal completion do not turn a tool error into task failure', () => {
    const failed = attempt('web_fetch', 2, 3, true, { code: 'connection_timeout' })
    expect(toolAttemptState(failed, [failed], 4, 'running')).toBe('continued')
    expect(toolAttemptState(failed, [failed], 4, 'completed')).toBe('continued')
    expect(toolAttemptReason(failed.output)).toBe('environment')
  })

  test('terminal failure and cancellation remain distinct from permission denial', () => {
    const failed = attempt('run_command', 2, 3, true, { error: { code: 'approval_denied' } })
    expect(toolAttemptState(failed, [failed], 3, 'running')).toBe('denied')
    expect(toolAttemptReason(failed.output)).toBe('permission')
    const blocked = attempt('read_file', 4, 5, true, { code: 'io_error' })
    expect(toolAttemptState(blocked, [blocked], 5, 'failed')).toBe('blocked')
    expect(toolAttemptState(blocked, [blocked], 5, 'cancelled')).toBe('cancelled')
  })
})
