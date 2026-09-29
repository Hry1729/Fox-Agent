import { expect, test } from 'bun:test'
import { ProcessScrollFollow, processScrollMetrics } from '../src/features/chat/process-scroll-follow'

function port(initialHeight = 1000) {
  const calls: ScrollToOptions[] = []
  const state = {
    scrollTop: 0,
    clientHeight: 400,
    scrollHeight: initialHeight,
    scrollTo(options: ScrollToOptions) {
      calls.push(options)
      if (options.behavior !== 'smooth') this.scrollTop = options.top ?? 0
    },
  }
  return { element: state as unknown as HTMLElement, state, calls }
}

test('each process group owns its own follow intent and a completed short group stays at the top', () => {
  const first = port(300)
  const second = port(1000)
  const completed = new ProcessScrollFollow()
  const live = new ProcessScrollFollow()
  completed.initialize(first.element, 'top')
  live.initialize(second.element, 'bottom')
  expect(completed.active).toBe(false)
  expect(live.active).toBe(true)
  expect(second.state.scrollTop).toBe(600)

  first.state.scrollHeight = 1200
  completed.followGrowth(first.element)
  completed.sample(processScrollMetrics(first.element))
  expect(first.calls).toHaveLength(0)
  expect(completed.active).toBe(false)
  first.state.scrollTop = 800
  completed.sample(processScrollMetrics(first.element))
  expect(completed.active).toBe(true)
  first.state.scrollHeight = 1300
  completed.followGrowth(first.element)
  expect(first.calls).toEqual([{ top: 900, behavior: 'smooth' }])
  expect(second.calls).toHaveLength(0)
})

test('native progress does not release follow or reissue targets until scrollend resamples growth', () => {
  const { element, state, calls } = port()
  const follow = new ProcessScrollFollow()
  follow.initialize(element, 'bottom')
  state.scrollHeight = 1200
  follow.followGrowth(element)
  expect(calls).toEqual([{ top: 800, behavior: 'smooth' }])
  state.scrollTop = 700
  follow.sample(processScrollMetrics(element))
  state.scrollHeight = 1400
  follow.followGrowth(element)
  expect(follow.active).toBe(true)
  expect(calls).toHaveLength(1)
  state.scrollTop = 800
  follow.settle(processScrollMetrics(element))
  follow.followGrowth(element)
  expect(calls).toEqual([
    { top: 800, behavior: 'smooth' },
    { top: 1000, behavior: 'smooth' },
  ])
})

test('reader input interrupts native motion; moving away releases follow and returning restores it', () => {
  const { element, state, calls } = port()
  const follow = new ProcessScrollFollow()
  follow.initialize(element, 'bottom')
  state.scrollHeight = 1200
  follow.followGrowth(element)
  state.scrollTop = 650
  follow.interrupt(element)
  expect(calls.at(-1)).toEqual({ top: 650, behavior: 'instant' })
  expect(follow.animating).toBe(false)
  state.scrollTop = 250
  follow.sample(processScrollMetrics(element))
  expect(follow.active).toBe(false)
  state.scrollHeight = 1400
  follow.followGrowth(element)
  expect(calls).toHaveLength(2)
  state.scrollTop = 1000
  follow.sample(processScrollMetrics(element))
  expect(follow.active).toBe(true)
  state.scrollHeight = 1500
  follow.followGrowth(element)
  expect(calls.at(-1)).toEqual({ top: 1100, behavior: 'smooth' })
})

test('a temporary uncapped mode restores the saved reader offset and follow state', () => {
  const { element, state } = port()
  const follow = new ProcessScrollFollow()
  follow.initialize(element, 'bottom')
  state.scrollTop = 220
  follow.sample(processScrollMetrics(element))
  const saved = state.scrollTop
  state.scrollTop = 0 // overflow: visible can clear the scrollport's offset
  state.scrollHeight = 1200
  follow.restore(element, saved)
  expect(state.scrollTop).toBe(220)
  expect(follow.active).toBe(false)

  state.scrollTop = 800
  follow.sample(processScrollMetrics(element))
  state.scrollTop = 0
  state.scrollHeight = 1400
  follow.restore(element, 800)
  expect(state.scrollTop).toBe(1000)
  expect(follow.active).toBe(true)
  follow.reset()
  expect(follow.active).toBe(false)
})

test('reduced motion follows growth immediately', () => {
  const original = globalThis.matchMedia
  globalThis.matchMedia = (() => ({ matches: true })) as typeof matchMedia
  try {
    const { element, state, calls } = port()
    const follow = new ProcessScrollFollow()
    follow.initialize(element, 'bottom')
    state.scrollHeight = 1200
    follow.followGrowth(element)
    expect(state.scrollTop).toBe(800)
    expect(calls).toHaveLength(0)
    expect(follow.animating).toBe(false)
  } finally {
    globalThis.matchMedia = original
  }
})
