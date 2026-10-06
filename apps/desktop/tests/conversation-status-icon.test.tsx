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

test('reduced motion stops the completion breathing without changing its glyph', async () => {
  const listeners=new Set<()=>void>()
  const media={matches:false,addEventListener:(_name:string,listener:()=>void)=>listeners.add(listener),removeEventListener:(_name:string,listener:()=>void)=>listeners.delete(listener)}
  const original=window.matchMedia
  window.matchMedia=(()=>media) as any
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<ConversationStatusIcon state="complete"/>))
    const positions=[...host.querySelectorAll('.fox-status-check-dot')].map(dot=>dot.getAttribute('style'))
    expect(host.querySelector('.fox-status-matrix')?.classList.contains('is-animated')).toBe(true)
    await act(async()=>{media.matches=true;listeners.forEach(listener=>listener())})
    expect(host.querySelector('.fox-status-matrix')?.classList.contains('is-animated')).toBe(false)
    expect([...host.querySelectorAll('.fox-status-check-dot')].map(dot=>dot.getAttribute('style'))).toEqual(positions)
  } finally {await act(async()=>root.unmount());window.matchMedia=original}
})
