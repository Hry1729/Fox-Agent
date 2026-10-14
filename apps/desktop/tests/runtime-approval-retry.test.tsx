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
    expect(host.querySelector('.aui-approval-operation')?.textContent).toContain('echo test')
  } finally {await act(async()=>root.unmount())}
})

test('a path-only permission uses the declared decision intersection and submits the original ticket', async () => {
  const calls: unknown[]=[]
  const approval={id:'kernel-approval:read:v9',status:'pending',toolName:'read_file',requestedAction:'读取文件',request:{authority:'kernel',category:'tool_execution',input:{path:'D:/project/数据.csv'},availableDecisions:['deny','allow_conversation']}} as ApprovalRecord
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<RuntimeApprovalPrompt approval={approval} onResolve={async(id,decision)=>{calls.push([id,decision]);return true}}/>))
    expect(host.querySelectorAll('[data-slot="permission-grant"]').length).toBe(1)
    expect(host.querySelector('[data-slot="approval-card"]')).toBeNull()
    const buttons=[...host.querySelectorAll<HTMLButtonElement>('button')]
    expect(buttons.map(button=>button.textContent)).toEqual(['拒绝','本对话内允许'])
    await act(async()=>buttons[1].click())
    expect(calls).toEqual([['kernel-approval:read:v9','allow_conversation']])
    expect(buttons.every(button=>button.disabled)).toBe(true)
  } finally {await act(async()=>root.unmount())}
})

test('ordinary writes keep content and all declared choices in one ApprovalCard', async () => {
  const calls: unknown[]=[]
  const approval={id:'kernel-approval:write:v8',status:'pending',toolName:'write_file',requestedAction:'写入文件',request:{authority:'kernel',category:'tool_execution',input:{path:'D:/project/报告.md',content:Array.from({length:426},(_,i)=>`内容 ${i}`).join('\n')},availableDecisions:['deny','allow_once','allow_conversation']}} as ApprovalRecord
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<RuntimeApprovalPrompt approval={approval} onResolve={async(id,decision)=>{calls.push([id,decision]);return true}}/>))
    expect(host.querySelectorAll('[data-slot="approval-card"]').length).toBe(1)
    expect(host.querySelector('[data-slot="permission-grant"]')).toBeNull()
    expect(host.querySelector('[aria-label="拟写入内容"]')?.textContent).toBe(approval.request.input && (approval.request.input as any).content)
    expect(host.querySelector<HTMLDetailsElement>('.fox-approval-details-scroll')?.open).toBe(false)
    expect(host.textContent?.split('D:/project/报告.md').length).toBe(2)
    expect(host.querySelectorAll('.aui-ink-button').length).toBe(1)
    expect(host.querySelector('.aui-ink-button')?.textContent).toBe('只允许这一次')
    expect([...host.querySelectorAll('button')].map(button=>button.textContent)).toEqual(['拒绝','本对话内允许','只允许这一次'])
    expect(host.querySelector('.aui-approval-operation')?.textContent).toBe('D:/project/报告.md')
    expect(host.querySelector('[data-slot="approval-card"] > div:first-child')?.textContent).not.toContain('D:/project/报告.md')
    const button=[...host.querySelectorAll<HTMLButtonElement>('button')].find(button=>button.textContent==='本对话内允许')!
    await act(async()=>button.click())
    expect(calls).toEqual([['kernel-approval:write:v8','allow_conversation']])
  } finally {await act(async()=>root.unmount())}
})

test('an existing-file replacement keeps the exact target and overwrite warning visible outside its collapsed details', async()=>{
  const target='D:/project/报告.md'
  const approval={id:'bound-ticket:v7',toolName:'write_file',requestedAction:'write_file',request:{target:'报告',input:{path:target,content:'完整内容'},availableDecisions:['deny','allow_once','allow_conversation'],wholeFileReplacement:{purpose:'整文件替换',targetIdentity:target,baselineVersion:'sha256:old',requestDigest:'sha256:request',candidateDigest:'sha256:new'}}} as any
  const host=document.createElement('div'),root=createRoot(host)
  try{
    await act(async()=>root.render(<RuntimeApprovalPrompt approval={approval} onResolve={async()=>true}/>))
    const card=host.querySelector('[data-slot="approval-card"]')!
    const operation=card.querySelector('.aui-approval-operation')!
    expect(operation.textContent).toBe(target)
    expect(operation.closest('details')).toBeNull()
    const description=card.querySelector(':scope > p')!
    expect(description.textContent).toContain('替换全部内容')
    expect(description.closest('details')).toBeNull()
    expect(host.querySelector<HTMLDetailsElement>('.fox-approval-details-scroll')?.open).toBe(false)
    expect([...host.querySelectorAll('button')].map(button=>button.textContent)).toEqual(['拒绝','只允许这一次'])
    expect(host.querySelector('.fox-approval-replacement')?.textContent).toContain('sha256:old')
  }finally{await act(async()=>root.unmount())}
})

test('unknown or malformed permission decisions expose only deny', async () => {
  for(const request of [{category:'other',availableDecisions:['deny','allow_once']},{category:'tool_execution',availableDecisions:['always']}]) {
    const host=document.createElement('div'),root=createRoot(host)
    const calls: unknown[]=[]
    try {
      await act(async()=>root.render(<RuntimeApprovalPrompt approval={{id:'same-ticket',toolName:'read_file',request:{...request,input:{path:'D:/project/read.md'}}} as any} onResolve={async(id,decision)=>{calls.push([id,decision]);return true}}/>))
      const buttons=[...host.querySelectorAll<HTMLButtonElement>('button')]
      expect(buttons.map(button=>button.textContent)).toEqual(['拒绝'])
      await act(async()=>buttons[0].click())
      expect(calls).toEqual([['same-ticket','deny']])
    } finally {await act(async()=>root.unmount())}
  }
})
