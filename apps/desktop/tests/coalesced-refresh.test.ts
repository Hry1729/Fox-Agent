import { describe, expect, test } from 'bun:test'
import { createCoalescedRefresh } from '../src/features/conversations/model/coalesced-refresh'

function scheduler() {
  const jobs: Array<{ callback: () => void; cancelled: boolean }> = []
  return {
    schedule: (callback: () => void) => { const job = { callback, cancelled: false }; jobs.push(job); return () => { job.cancelled = true } },
    fire: async () => { const job = jobs.shift(); if (job && !job.cancelled) job.callback(); await Promise.resolve(); await Promise.resolve() },
    count: () => jobs.filter((job) => !job.cancelled).length,
  }
}

describe('Kernel invalidation refresh', () => {
  test('coalesces a burst and retains a trailing invalidation during an in-flight load', async () => {
    const clock = scheduler()
    let calls = 0
    const resolutions: Array<(value: number) => void> = []
    const applied: number[] = []
    const refresh = createCoalescedRefresh({ schedule: clock.schedule,
      load: () => { calls++; return new Promise<number>((resolve) => resolutions.push(resolve)) },
      apply: (value) => applied.push(value), fail: () => { throw new Error('unexpected failure') },
    })
    for (let i = 0; i < 10_000; i++) refresh.invalidate()
    expect(clock.count()).toBe(1)
    await clock.fire()
    for (let i = 0; i < 10_000; i++) refresh.invalidate()
    expect(calls).toBe(1)
    expect(clock.count()).toBe(0)
    resolutions.shift()!(1)
    await Promise.resolve()
    expect(clock.count()).toBe(1)
    await clock.fire()
    resolutions.shift()!(2)
    await Promise.resolve()
    expect(applied).toEqual([1, 2])
    expect(calls).toBe(2)
    expect(clock.count()).toBe(0)
    refresh.dispose()
  })

  test('retries at most three times, reports failure, and accepts a later recovery hint', async () => {
    const clock = scheduler()
    let calls = 0
    const errors: unknown[] = []
    const applied: number[] = []
    const refresh = createCoalescedRefresh({ schedule: clock.schedule,
      load: async () => { if (++calls <= 3) throw new Error('offline'); return calls },
      apply: (value) => applied.push(value), fail: (error) => errors.push(error),
    })
    refresh.invalidate()
    await clock.fire(); await clock.fire(); await clock.fire()
    expect(calls).toBe(3)
    expect(errors.length).toBe(1)
    expect(clock.count()).toBe(0)
    refresh.invalidate()
    await clock.fire()
    expect(applied).toEqual([4])
    refresh.dispose()
  })

  test('dispose cancels queued work and fences late results and failures', async () => {
    for (const reject of [false, true]) {
      const clock = scheduler()
      let settle!: () => void
      let effects = 0
      const refresh = createCoalescedRefresh({ schedule: clock.schedule,
        load: () => new Promise<number>((resolve, fail) => { settle = () => reject ? fail(new Error('old conversation')) : resolve(1) }),
        apply: () => effects++, fail: () => effects++,
      })
      refresh.invalidate()
      await clock.fire()
      refresh.invalidate()
      refresh.dispose()
      settle()
      await Promise.resolve()
      expect(effects).toBe(0)
      expect(clock.count()).toBe(0)
      refresh.invalidate()
      expect(clock.count()).toBe(0)
    }
    const clock = scheduler()
    let called = false
    const refresh = createCoalescedRefresh({ schedule: clock.schedule,
      load: async () => { called = true }, apply: () => {}, fail: () => {},
    })
    refresh.invalidate(); refresh.dispose(); await clock.fire()
    expect(called).toBe(false)
  })
})
