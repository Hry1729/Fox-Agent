import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { RuntimeApprovalPrompt } from '../src/features/chat/components/RuntimeApprovalPrompt'
import type { ApprovalRecord } from '../src/features/conversations/model/types'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

test('a rejected resolver re-enables the exact ticket buttons for retry', async () => {
  const approval={id:'original-ticket:v7',status:'pending',toolName:'run_command',request:{tool:'run_command',input:{command:'echo test'},availableDecisions:['deny','allow_once']}} as ApprovalRecord
  const calls:string[]=[]
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<RuntimeApprovalPrompt approval={approval} onResolve={async id=>{calls.push(id);throw new Error('临时连接失败')}}/>))
    const button=()=>[...host.querySelectorAll<HTMLButtonElement>('button')].find(button=>button.textContent?.includes('只允许这一次'))!
    await act(async()=>button().click())
    expect(button().disabled).toBe(false)
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('临时连接失败')
    await act(async()=>button().click())
    expect(calls).toEqual(['original-ticket:v7','original-ticket:v7'])
    expect(host.querySelector('.fox-confirmation-actions')?.parentElement).toBe(host.querySelector('.fox-runtime-confirmation'))
    expect(host.querySelector('.fox-approval-details-scroll .fox-approval-command')).not.toBeNull()
  } finally {await act(async()=>root.unmount())}
})
