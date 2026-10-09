import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import {
  ConversationStatusIcon, GLYPH_CYCLE_MS, STATUS_BREATHE_MS, STATUS_RING_FORMS,
  STATUS_RING_HOLD_PERCENT, STATUS_RING_MORPH_MS, STATUS_RING_MORPH_PERCENT,
  STATUS_RING_STAGGER_STEPS, statusRingMorphDeadlinePercent, statusRingMorphStartPercent,
} from '../src/features/chat/conversation-status-icon'
import { GLYPH_FORM_COUNT, GLYPH_HOLD_RATIO, GLYPH_MORPH_RATIO, glyphMorphProgress, glyphMorphWindowMs } from '../src/components/dotmatrix/dotm-3x3-11'

if (typeof document === 'undefined') GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

test('the settled ring runs at the running glyph tempo, 7 cycles to 2', () => {
  // The blue glyph paints seven shapes per cycle and the green/yellow ring
  // alternates between two states, so one form must cost the same time in both.
  expect(GLYPH_FORM_COUNT).toBe(7)
  expect(STATUS_RING_FORMS).toBe(2)
  expect(GLYPH_CYCLE_MS / GLYPH_FORM_COUNT).toBeCloseTo(STATUS_BREATHE_MS / STATUS_RING_FORMS, 6)
  expect(GLYPH_CYCLE_MS / STATUS_BREATHE_MS).toBeCloseTo(7 / 2, 6)
})

test('the ring changes over the same window the glyph morphs in', () => {
  // Equal cycle ratios are not enough: the glyph holds each shape and morphs for
  // 34% of a beat, so the ring has to spend that same time changing.
  expect(STATUS_RING_MORPH_MS).toBeCloseTo(glyphMorphWindowMs(.65), 6)
  expect(STATUS_RING_MORPH_MS).toBeCloseTo(201.8, 1)
  expect(STATUS_RING_MORPH_PERCENT).toBeCloseTo(17, 6)
  expect(STATUS_RING_HOLD_PERCENT).toBeCloseTo(26, 6)
})

test('every stagger step keeps the glyph rule: later start, one shared deadline', async () => {
  const css = await Bun.file(new URL('../src/features/chat/conversation-status-icon.css', import.meta.url)).text()
  const deadline = statusRingMorphDeadlinePercent()
  const secondDeadline = deadline + 100 / STATUS_RING_FORMS
  // One keyframe set per stagger step, sliced to that set only (steps 0-3 are one
  // line each; step 4 carries the per-segment step function).
  const blockFor = (step: number) => {
    const start = css.indexOf(`@keyframes foxStatusRingDot${step} `)
    expect(start).toBeGreaterThan(-1)
    const end = step === 4 ? css.indexOf('\n}', start) + 2 : css.indexOf('\n', start)
    return css.slice(start, end)
  }
  for (let step = 0; step < STATUS_RING_STAGGER_STEPS; step += 1) {
    const stagger = step / (STATUS_RING_STAGGER_STEPS - 1)
    const start = statusRingMorphStartPercent(stagger)
    const block = blockFor(step)
    // Later dots start their change later, never earlier.
    expect(start).toBeGreaterThanOrEqual(STATUS_RING_HOLD_PERCENT)
    if (step > 0) expect(start).toBeGreaterThan(statusRingMorphStartPercent((step - 1) / (STATUS_RING_STAGGER_STEPS - 1)))
    // No duplicated offset may carry two different values: CSS keeps only the last
    // declaration for a repeated offset, so such a rule is not the step it looks like.
    const selectors = Array.from(block.matchAll(/([\d.,\s%]+)\s*\{/g)).flatMap((match) => (match[1] || '').split(',').map((part) => part.trim()).filter(Boolean))
    expect(new Set(selectors).size, `duplicate keyframe offset in foxStatusRingDot${step}`).toBe(selectors.length)
    expect(block).toContain(`${start}%`)
    expect(block).toContain(`${deadline}%`)
    expect(block).toContain(`${secondDeadline}%`)
  }
  // The last step must snap, and only an explicit step function does that.
  const step4 = blockFor(4)
  expect(step4).toContain('steps(1, end)')
  expect((step4.match(/steps\(1, end\)/g) || []).length).toBe(2)
  expect(step4).toContain('0% { opacity: 0;')
  expect(step4).toContain(`${deadline}% { opacity: .95;`)

  // A delay would move the end too, so no dot may carry one.
  const host = document.createElement('div'), root = createRoot(host)
  try {
    await act(async () => root.render(<ConversationStatusIcon state="complete" />))
    const dots = Array.from(host.querySelectorAll<HTMLElement>('.fox-status-ring-dot'))
    expect(dots.length).toBe(9)
    expect(dots.every((dot) => !dot.style.animationDelay)).toBe(true)
    expect(dots.map((dot) => dot.className.match(/is-stagger-(\d)/)?.[1]).filter(Boolean).length).toBe(9)
    // row+column ordering, exactly like the glyph.
    expect(dots[0]?.className).toContain('is-stagger-0')
    expect(dots[8]?.className).toContain('is-stagger-4')
  } finally { await act(async () => root.unmount()) }
})

test('the shared per-dot rule starts later dots later and finishes them together', () => {
  const hold = GLYPH_HOLD_RATIO
  const morph = GLYPH_MORPH_RATIO
  const deadline = hold + morph
  // Half way through the window: the unstaggered dot is part way through its
  // change while the furthest dot has not started at all.
  const middle = hold + morph / 2
  expect(glyphMorphProgress(middle, 0)).toBeGreaterThan(0)
  expect(glyphMorphProgress(middle, 0.5)).toBeGreaterThan(0)
  expect(glyphMorphProgress(middle, 1)).toBe(0)
  for (const stagger of [0, 0.25, 0.5, 0.75, 1]) {
    // Nothing has started before its own hold ends…
    expect(glyphMorphProgress(hold + stagger * morph - 0.001, stagger)).toBe(0)
    // …and every dot is finished at the one shared deadline.
    expect(glyphMorphProgress(deadline, stagger)).toBe(1)
    expect(glyphMorphProgress(deadline + 0.05, stagger)).toBe(1)
  }
  // A later dot never starts earlier than an earlier one.
  for (const phase of [hold + 0.05, hold + morph * 0.5, deadline - 0.01]) {
    let previous = -1
    for (const stagger of [0, 0.25, 0.5, 0.75, 1]) {
      const progress = glyphMorphProgress(phase, stagger)
      expect(progress).toBeLessThanOrEqual(previous < 0 ? 1 : previous + 1e-9)
      previous = progress
    }
  }
  // The same rule reaches the ring through its keyframe percentages.
  expect(statusRingMorphStartPercent(1)).toBeCloseTo(deadline * 100 / STATUS_RING_FORMS * STATUS_RING_FORMS / 2, 6)
  expect(glyphMorphProgress(hold + morph, 1)).toBe(1)
})

test('every state pins its breathe to the shared tempo', async () => {
  const host = document.createElement('div'), root = createRoot(host)
  try {
    for (const state of ['waiting', 'complete'] as const) {
      await act(async () => root.render(<ConversationStatusIcon state={state} />))
      const matrix = host.querySelector<HTMLElement>('.fox-status-matrix')
      expect(matrix?.style.getPropertyValue('--fox-status-breathe')).toBe(`${STATUS_BREATHE_MS}ms`)
    }
  } finally { await act(async () => root.unmount()) }
})

test('idle reserves no painted icon and every active state keeps the nine-cell square', async () => {
  const original=window.matchMedia
  window.matchMedia=(()=>({matches:true,addEventListener:()=>{},removeEventListener:()=>{}})) as any
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<ConversationStatusIcon state="idle"/>))
    expect(host.childElementCount).toBe(0)
    for (const state of ['running','waiting','complete'] as const){
      await act(async()=>root.render(<ConversationStatusIcon state={state}/>))
      expect(host.querySelectorAll('.dmx-dot, .fox-status-ring-dot').length).toBe(9)
      expect(host.querySelector('[data-icon-state]')?.getAttribute('data-icon-state')).toBe(state)
    }
  } finally {await act(async()=>root.unmount());window.matchMedia=original}
})

test('reduced motion stops Glyph Pulse and retains its nine-dot layout', async () => {
  const listeners=new Set<()=>void>()
  const media={matches:false,addEventListener:(_name:string,listener:()=>void)=>listeners.add(listener),removeEventListener:(_name:string,listener:()=>void)=>listeners.delete(listener)}
  const original=window.matchMedia
  const frames=new Map<number,FrameRequestCallback>()
  let sequence=0
  const originalRequest=window.requestAnimationFrame,originalCancel=window.cancelAnimationFrame
  window.requestAnimationFrame=callback=>{const id=++sequence;frames.set(id,callback);return id}
  window.cancelAnimationFrame=id=>{frames.delete(id)}
  window.matchMedia=(()=>media) as any
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<ConversationStatusIcon state="running"/>))
    expect(host.querySelectorAll('.dmx-dot').length).toBe(9)
    expect(host.querySelector('.fox-status-matrix')?.classList.contains('is-animated')).toBe(true)
    await act(async()=>{media.matches=true;listeners.forEach(listener=>listener())})
    expect(host.querySelector('.fox-status-matrix')?.classList.contains('is-animated')).toBe(false)
    expect(host.querySelectorAll('.dmx-dot').length).toBe(9)
    expect(Array.from(host.querySelectorAll<HTMLElement>('.dmx-dot')).every(dot=>!dot.style.transition)).toBe(true)
    expect(frames.size).toBe(0)
  } finally {
    await act(async()=>root.unmount());window.matchMedia=original
    window.requestAnimationFrame=originalRequest;window.cancelAnimationFrame=originalCancel
  }
})

test('late running icons join Glyph Pulse at the same pattern and brightness', async () => {
  const frames=new Map<number,FrameRequestCallback>()
  let sequence=0
  const originalRequest=window.requestAnimationFrame,originalCancel=window.cancelAnimationFrame
  window.requestAnimationFrame=callback=>{const id=++sequence;frames.set(id,callback);return id}
  window.cancelAnimationFrame=id=>{frames.delete(id)}
  const host=document.createElement('div'),root=createRoot(host)
  const render=(late:boolean)=><><div data-copy="first"><ConversationStatusIcon state="running"/></div>{late&&<div data-copy="late"><ConversationStatusIcon state="running"/></div>}</>
  const opacities=(copy:string)=>Array.from(host.querySelectorAll<HTMLElement>(`[data-copy="${copy}"] .dmx-dot`)).map(dot=>dot.style.opacity)
  const advance=async(now:number)=>act(async()=>{const pending=Array.from(frames.values());frames.clear();pending.forEach(callback=>callback(now))})
  try {
    await act(async()=>root.render(render(false)))
    await advance(700)
    const before=opacities('first')
    await act(async()=>root.render(render(true)))
    await advance(1600)
    expect(opacities('late')).toEqual(opacities('first'))
    expect(opacities('first')).not.toEqual(before)
    expect(opacities('late')).toHaveLength(9)
    expect(frames.size).toBe(1)
  } finally {
    await act(async()=>root.unmount())
    expect(frames.size).toBe(0)
    window.requestAnimationFrame=originalRequest;window.cancelAnimationFrame=originalCancel
  }
})
