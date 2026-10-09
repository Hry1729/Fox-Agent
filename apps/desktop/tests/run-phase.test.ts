import { describe, expect, test } from 'bun:test'
import { runPhaseTiming, runPhaseTitle } from '../src/features/chat/run-phase'

type Event = Parameters<typeof runPhaseTiming>[0][number]
const event = (seq: number, eventType: string, data: Record<string, unknown> = {}, createdAt = seq * 1000): Event => ({
  runId: 'run-1', seq, eventType, event: { type: eventType, ...data }, createdAt,
})

describe('run phase from durable events', () => {
  test('a run without run.started is still queued, and reports the real queue wait', () => {
    const timing = runPhaseTiming([], { queuedAt: 1000, now: 586_000 })
    expect(timing.phase).toBe('queued')
    expect(timing.startedAt).toBeNull()
    // 585 seconds waiting for an execution slot, as observed in the production runs.
    expect(timing.queueMs).toBe(585_000)
    expect(timing.executionMs).toBeNull()
    // The label is the phase alone: the run's single timer sits beside the avatar,
    // so gluing the same interval into this line reported it twice.
    expect(runPhaseTitle(timing)).toBe('准备执行')
  })

  test('an unknown enqueue time stays unknown instead of being estimated', () => {
    const timing = runPhaseTiming([], { now: 5000 })
    expect(timing.phase).toBe('queued')
    expect(timing.queueMs).toBeNull()
    expect(timing.executionMs).toBeNull()
    expect(runPhaseTitle(timing)).toBe('准备执行')
  })

  test('after run.started the phases are distinguished and the waits are split', () => {
    const started = event(1, 'run.started', {}, 2000)
    const waiting = runPhaseTiming([started], { queuedAt: 1000, now: 38_000 })
    // Started executing, nothing readable yet: that is a model wait, not "queued".
    expect(waiting.phase).toBe('awaiting_response')
    expect(waiting.queueMs).toBe(1000)
    expect(waiting.executionMs).toBe(36_000)
    // The wait is still reported — as the timing facts above, not as a second timer
    // glued into the status line.
    expect(runPhaseTitle(waiting)).toBe('等待模型响应')

    const streaming = runPhaseTiming(
      [started, event(2, 'reasoning.delta', { delta: '先分析' }, 3000)],
      { queuedAt: 1000, now: 10_000 },
    )
    expect(streaming.phase).toBe('streaming')
    expect(runPhaseTitle(streaming)).toBe('正在生成')
  })

  test('a streamed answer segment counts as generation, not as a wait', () => {
    const timing = runPhaseTiming(
      [event(1, 'run.started', {}, 2000), event(2, 'message.started', {}, 2500)],
      { queuedAt: 1000, now: 9000 },
    )
    expect(timing.phase).toBe('streaming')
  })

  test('a durable run.phase event refines the stage without inventing one', () => {
    const events = [event(1, 'run.started', {}, 2000), event(2, 'run.phase', { phase: 'preparing' }, 2100)]
    expect(runPhaseTiming(events, { queuedAt: 1000, now: 4000 }).phase).toBe('preparing')
    expect(runPhaseTiming([...events, event(3, 'run.phase', { phase: 'request_sent' }, 2200)], { now: 4000 }).phase).toBe('awaiting_response')
    expect(runPhaseTiming([...events, event(3, 'run.phase', { phase: 'finalizing' }, 2200)], { now: 4000 }).phase).toBe('finalizing')
    // An unknown phase never becomes a stage of its own.
    expect(runPhaseTiming([...events, event(3, 'run.phase', { phase: 'unheard-of' }, 2200)], { now: 4000 }).phase).toBe('preparing')
  })

  test('a terminal event stops the clock for both waits', () => {
    const timing = runPhaseTiming(
      [event(1, 'run.started', {}, 2000), event(2, 'run.completed', {}, 5000)],
      { queuedAt: 1000, now: 9000 },
    )
    expect(timing.phase).toBe('settled')
    expect(timing.queueMs).toBe(1000)
    // Frozen at the terminal event, not still counting towards `now`.
    expect(timing.executionMs).toBe(3000)
  })

  test('a cancelled run that never started is settled, and never reports execution', () => {
    const timing = runPhaseTiming([event(1, 'run.cancelled', {}, 4000)], { queuedAt: 1000, now: 9000 })
    expect(timing.phase).toBe('settled')
    expect(timing.startedAt).toBeNull()
    expect(timing.executionMs).toBeNull()
    expect(timing.queueMs).toBe(3000)
  })

  test('the earliest run.started wins when events arrive out of order', () => {
    const timing = runPhaseTiming(
      [event(2, 'run.started', {}, 5000), event(1, 'run.started', {}, 2000)],
      { queuedAt: 1000, now: 6000 },
    )
    expect(timing.startedAt).toBe(2000)
    expect(timing.queueMs).toBe(1000)
  })

  test('a queue wait is never negative when clocks disagree', () => {
    const timing = runPhaseTiming([event(1, 'run.started', {}, 900)], { queuedAt: 1000, now: 5000 })
    expect(timing.queueMs).toBe(0)
  })
})
