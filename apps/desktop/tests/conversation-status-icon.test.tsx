import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { ConversationStatusIcon } from '../src/features/chat/conversation-status-icon'

if (typeof document === 'undefined') GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

test('idle reserves no painted icon while running and waiting use nine dots', async () => {
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<ConversationStatusIcon state="idle"/>))
    expect(host.childElementCount).toBe(0)
    for (const state of ['running','waiting'] as const){
      await act(async()=>root.render(<ConversationStatusIcon state={state}/>))
      expect(host.querySelectorAll('.dmx-dot').length).toBe(9)
      expect(host.querySelector('[data-icon-state]')?.getAttribute('data-icon-state')).toBe(state)
    }
  } finally {await act(async()=>root.unmount())}
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
    await act(async()=>root.render(<ConversationStatusIcon state="complete"/>))
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

test('late completion icons join Glyph Pulse at the same pattern and brightness', async () => {
  const frames=new Map<number,FrameRequestCallback>()
  let sequence=0
  const originalRequest=window.requestAnimationFrame,originalCancel=window.cancelAnimationFrame
  window.requestAnimationFrame=callback=>{const id=++sequence;frames.set(id,callback);return id}
  window.cancelAnimationFrame=id=>{frames.delete(id)}
  const host=document.createElement('div'),root=createRoot(host)
  const render=(late:boolean)=><><div data-copy="first"><ConversationStatusIcon state="complete"/></div>{late&&<div data-copy="late"><ConversationStatusIcon state="complete"/></div>}</>
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
