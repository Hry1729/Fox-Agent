import { describe, expect, it } from 'bun:test'
import {
  canViewResult,
  isCancellable,
  isTerminal,
  progressRatio,
  progressText,
  stateLabel,
  type JobSnapshot,
} from '../src/features/jobs/job-presentation'

function snapshot(over: Partial<JobSnapshot>): JobSnapshot {
  return {
    jobId: 'job-12345678-abcd',
    runId: 'run-1',
    kind: 'attachment_compute',
    state: 'running',
    progressDone: 0,
    startedAt: 1,
    updatedAt: 2,
    attempts: 1,
    ...over,
  }
}

describe('job presentation', () => {
  it('classifies terminal and cancellable states', () => {
    expect(isTerminal('completed')).toBe(true)
    expect(isTerminal('failed')).toBe(true)
    expect(isTerminal('cancelled')).toBe(true)
    expect(isTerminal('running')).toBe(false)
    expect(isCancellable('queued')).toBe(true)
    expect(isCancellable('running')).toBe(true)
    expect(isCancellable('paused')).toBe(true)
    expect(isCancellable('completed')).toBe(false)
  })

  it('computes a bounded ratio only when the total is known', () => {
    expect(progressRatio(snapshot({ progressDone: 50, progressTotal: 200 }))).toBe(0.25)
    expect(progressRatio(snapshot({ progressDone: 300, progressTotal: 200 }))).toBe(1)
    expect(progressRatio(snapshot({ progressDone: 10, progressTotal: null }))).toBeNull()
    expect(progressRatio(snapshot({ progressDone: 10, progressTotal: 0 }))).toBeNull()
  })

  it('renders honest progress text', () => {
    expect(progressText(snapshot({ progressDone: 500, progressTotal: 1000 }))).toContain('50%')
    expect(progressText(snapshot({ progressDone: 500, progressTotal: null }))).toContain('500')
    expect(progressText(snapshot({ state: 'completed', progressDone: 500 }))).toBe('计算完成')
    expect(progressText(snapshot({ state: 'queued', progressDone: 0 }))).toBe(stateLabel('queued'))
  })

  it('only offers the result view for completed jobs with a reference', () => {
    expect(canViewResult(snapshot({ state: 'completed', resultRef: 'fox-result://run-1/call-1' }))).toBe(true)
    expect(canViewResult(snapshot({ state: 'completed', resultRef: null }))).toBe(false)
    expect(canViewResult(snapshot({ state: 'running', resultRef: 'fox-result://run-1/call-1' }))).toBe(false)
  })
})
