import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import type { ConversationSummary } from '../src/features/conversations/model/types'
import { EMPTY_CONVERSATION_RUN_OVERLAY, applyApprovalToOverlay, conversationRunActivity } from '../src/features/chat/conversation-run-indicator'
import { COMPLETION_NOTICES_KEY, useConversationCompletionNotices, useVisibleCompletionAcknowledgement } from '../src/features/chat/use-conversation-completion-notices'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const summary = (id: string, runId: string): ConversationSummary => ({ id, title:id, agentId:'fox-general', agentName:'Fox', projectId:null, projectRoot:null, status:'active', createdAt:1, updatedAt:1, lastMessageAt:1, lastRunId:runId, lastRunStatus:'completed' })

test('acknowledgement hides only that completed Run, survives a reload, and a new Run notifies again', async () => {
  const values = new Map<string, string>()
  const storage = { getItem:(key:string)=>values.get(key)??null, setItem:(key:string,value:string)=>{values.set(key,value)} }
  let notices: ReturnType<typeof useConversationCompletionNotices>
  function Scene({ conversations }: {conversations: ConversationSummary[]}) {
    notices = useConversationCompletionNotices(conversations, EMPTY_CONVERSATION_RUN_OVERLAY, 10, storage)
    return null
  }
  const host=document.createElement('div')
  let root=createRoot(host)
  try {
    await act(async()=>root.render(<Scene conversations={[summary('a','run-1'),summary('b','run-b')]}/>))
    expect(notices!.runIndicators.get('a')).toBe('completed')
    await act(async()=>notices!.acknowledgeCompletion('a'))
    expect(notices!.runIndicators.has('a')).toBe(false)
    expect(notices!.runIndicators.get('b')).toBe('completed')
    expect(JSON.parse(values.get(COMPLETION_NOTICES_KEY)!)).toEqual([['a','run-1']])
    await act(async()=>root.unmount())
    root=createRoot(host)
    await act(async()=>root.render(<Scene conversations={[summary('a','run-1')]}/>))
    expect(notices!.runIndicators.has('a')).toBe(false)
    await act(async()=>root.render(<Scene conversations={[summary('a','run-2')]}/>))
    expect(notices!.runIndicators.get('a')).toBe('completed')
  } finally { await act(async()=>root.unmount()) }
})

test('a late approval cannot revive a finished Run, including after its overlay was pruned', () => {
  const completed=summary('a','run-1')
  const terminal = new Map([['a',{indicator:'completed' as const,runId:'run-1',updatedAt:10}]])
  expect(applyApprovalToOverlay(terminal,{conversationId:'a',runId:'run-1',status:'pending'},20)).toBe(terminal)
  const late=applyApprovalToOverlay(EMPTY_CONVERSATION_RUN_OVERLAY,{conversationId:'a',runId:'run-1',status:'pending'},20)
  expect(conversationRunActivity(completed,late,10).indicator).toBe('completed')
  const current=new Map([['a',{indicator:'approval' as const,runId:'run-2',updatedAt:20}]])
  expect(applyApprovalToOverlay(current,{conversationId:'a',runId:'run-1',status:'expired'},30)).toBe(current)
})

test('completion stays visible until an interaction or returning to that page', async () => {
  let notices: ReturnType<typeof useConversationCompletionNotices>
  const storage={getItem:()=>null,setItem:()=>{}}
  function Scene({completed,visible=true,runId='run-1'}:{completed:boolean;visible?:boolean;runId?:string}) {
    const item={...summary('a',runId),lastRunStatus:completed?'completed':'running',activeRunId:completed?null:runId}
    notices=useConversationCompletionNotices([item],EMPTY_CONVERSATION_RUN_OVERLAY,10,storage)
    const acknowledge=useVisibleCompletionAcknowledgement({visible,conversationId:'a',displayedConversationId:'a',acknowledgeCompletion:notices.acknowledgeCompletion})
    return <div onWheelCapture={acknowledge} onPointerDownCapture={acknowledge}/>
  }
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<Scene completed={false}/>))
    await act(async()=>root.render(<Scene completed/>))
    expect(notices!.runIndicators.get('a')).toBe('completed')
    await act(async()=>host.firstElementChild!.dispatchEvent(new window.WheelEvent('wheel',{bubbles:true})))
    expect(notices!.runIndicators.has('a')).toBe(false)
    await act(async()=>root.render(<Scene completed runId="run-2" visible={false}/>))
    expect(notices!.runIndicators.get('a')).toBe('completed')
    await act(async()=>root.render(<Scene completed runId="run-2" visible/>))
    expect(notices!.runIndicators.has('a')).toBe(false)
    await act(async()=>root.render(<Scene completed runId="run-3"/>))
    expect(notices!.runIndicators.get('a')).toBe('completed')
    await act(async()=>host.firstElementChild!.dispatchEvent(new window.PointerEvent('pointerdown',{bubbles:true})))
    expect(notices!.runIndicators.has('a')).toBe(false)
  } finally {await act(async()=>root.unmount())}
})

test('navigation does not acknowledge a selected conversation until its detail is displayed', async () => {
  let notices: ReturnType<typeof useConversationCompletionNotices>
  const storage={getItem:()=>null,setItem:()=>{}}
  function Scene({displayed}:{displayed:string}) {
    notices=useConversationCompletionNotices([summary('a','run-a'),summary('b','run-b')],EMPTY_CONVERSATION_RUN_OVERLAY,10,storage)
    useVisibleCompletionAcknowledgement({visible:true,conversationId:'a',displayedConversationId:displayed,acknowledgeCompletion:notices.acknowledgeCompletion})
    return null
  }
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>root.render(<Scene displayed="b"/>))
    expect(notices!.runIndicators.get('a')).toBe('completed')
    await act(async()=>root.render(<Scene displayed="a"/>))
    expect(notices!.runIndicators.has('a')).toBe(false)
    expect(notices!.runIndicators.get('b')).toBe('completed')
  } finally {await act(async()=>root.unmount())}
})
